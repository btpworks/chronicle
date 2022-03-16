use std::{collections::HashMap, str::FromStr, time::Duration};

use chrono::DateTime;

use chrono::Utc;
use common::prov::Attachment;
use common::prov::AttachmentId;
use common::prov::Identity;
use common::prov::IdentityId;
use common::{
    ledger::Offset,
    prov::{
        vocab::Chronicle, Activity, ActivityId, Agent, AgentId, Entity, EntityId, Namespace,
        NamespaceId, ProvModel,
    },
};
use custom_error::custom_error;
use derivative::*;

use diesel::{
    connection::SimpleConnection,
    dsl::max,
    prelude::*,
    r2d2::{ConnectionManager, Pool, PooledConnection},
    sqlite::SqliteConnection,
};
use diesel_migrations::{embed_migrations, EmbeddedMigrations};
use tracing::{debug, instrument, trace, warn};
use uuid::Uuid;

use crate::QueryCommand;

mod query;
pub(crate) mod schema;
pub const MIGRATIONS: EmbeddedMigrations = embed_migrations!();

custom_error! {pub StoreError
    Db{source: diesel::result::Error}                           = "Database operation failed",
    DbConnection{source: diesel::ConnectionError}               = "Database connection failed",
    DbMigration{migration: Box<dyn custom_error::Error + Send + Sync>} = "Database migration failed {:?}",
    DbPool{source: r2d2::Error}                                 = "Connection pool error",
    Uuid{source: uuid::Error}                                   = "Invalid UUID string",
    RecordNotFound{}                                            = "Could not locate record in store",
    InvalidNamespace{}                                          = "Could not find namespace",
    ModelDoesNotContainActivity{activityid: ActivityId}         = "Could not locate {} in activities",
    ModelDoesNotContainAgent{agentid: AgentId}                  = "Could not locate {} in agents",
    ModelDoesNotContainEntity{entityid: EntityId}               = "Could not locate {} in entities",
}

#[derive(Debug)]
pub struct ConnectionOptions {
    pub enable_wal: bool,
    pub enable_foreign_keys: bool,
    pub busy_timeout: Option<Duration>,
}

#[instrument]
fn sleeper(attempts: i32) -> bool {
    warn!(attempts, "SQLITE_BUSY, retrying");
    std::thread::sleep(std::time::Duration::from_millis(250));
    true
}

impl diesel::r2d2::CustomizeConnection<SqliteConnection, diesel::r2d2::Error>
    for ConnectionOptions
{
    fn on_acquire(&self, conn: &mut SqliteConnection) -> Result<(), diesel::r2d2::Error> {
        (|| {
            if self.enable_wal {
                conn.batch_execute(
                    r#"PRAGMA journal_mode = WAL2;
                PRAGMA synchronous = NORMAL;
                PRAGMA wal_autocheckpoint = 1000;
                PRAGMA wal_checkpoint(TRUNCATE);"#,
                )?;
            }
            if self.enable_foreign_keys {
                conn.batch_execute("PRAGMA foreign_keys = ON;")?;
            }
            if let Some(d) = self.busy_timeout {
                conn.batch_execute(&format!("PRAGMA busy_timeout = {};", d.as_millis()))?;
            }

            Ok(())
        })()
        .map_err(diesel::r2d2::Error::QueryError)
    }
}

#[derive(Derivative)]
#[derivative(Debug, Clone)]
pub struct Store {
    #[derivative(Debug = "ignore")]
    pool: Pool<ConnectionManager<SqliteConnection>>,
}

impl Store {
    /// Fetch the activity record for the IRI
    pub fn activity_by_activity_name_and_namespace(
        &self,
        connection: &mut SqliteConnection,
        name: &str,
        namespaceid: &NamespaceId,
    ) -> Result<query::Activity, StoreError> {
        let (_namespaceid, nsid) = self.namespace_by_name(connection, namespaceid.decompose().0)?;
        use schema::activity::dsl;

        Ok(schema::activity::table
            .filter(dsl::name.eq(name).and(dsl::namespace_id.eq(nsid)))
            .first::<query::Activity>(connection)?)
    }

    /// Fetch the agent record for the IRI
    pub(crate) fn agent_by_agent_name_and_namespace(
        &self,
        connection: &mut SqliteConnection,
        name: &str,
        namespaceid: &NamespaceId,
    ) -> Result<query::Agent, StoreError> {
        let (_namespaceid, nsid) = self.namespace_by_name(connection, namespaceid.decompose().0)?;
        use schema::agent::dsl;

        Ok(schema::agent::table
            .filter(dsl::name.eq(name).and(dsl::namespace_id.eq(nsid)))
            .first::<query::Agent>(connection)?)
    }

    /// Apply an activity to persistent storage, name + namespace are a key, so we update times + domaintype on conflict
    #[instrument(skip(connection))]
    fn apply_activity(
        &self,
        connection: &mut SqliteConnection,
        Activity {
            ref name,
            id,
            namespaceid,
            started,
            ended,
            domaintypeid,
        }: &Activity,
        ns: &HashMap<NamespaceId, Namespace>,
    ) -> Result<(), StoreError> {
        use schema::activity::{self as dsl};
        let _namespace = ns.get(namespaceid).ok_or(StoreError::InvalidNamespace {})?;
        let (_, nsid) = self.namespace_by_name(connection, namespaceid.decompose().0)?;

        diesel::insert_into(schema::activity::table)
            .values((
                dsl::name.eq(name),
                dsl::namespace_id.eq(nsid),
                dsl::started.eq(started.map(|t| t.naive_utc())),
                dsl::ended.eq(ended.map(|t| t.naive_utc())),
                dsl::domaintype.eq(domaintypeid.as_ref().map(|x| x.decompose())),
            ))
            .on_conflict((dsl::name, dsl::namespace_id))
            .do_update()
            .set((
                dsl::started.eq(started.map(|t| t.naive_utc())),
                dsl::ended.eq(ended.map(|t| t.naive_utc())),
            ))
            .execute(connection)?;

        Ok(())
    }

    /// Apply an agent to persistent storage, name + namespace are a key, so we update publickey + domaintype on conflict
    /// current is a special case, only relevent to local CLI context. A possibly improved design would be to store this in another table given its scope
    #[instrument(skip(connection))]
    fn apply_agent(
        &self,
        connection: &mut SqliteConnection,
        Agent {
            ref name,
            namespaceid,
            id: _,
            domaintypeid,
        }: &Agent,
        ns: &HashMap<NamespaceId, Namespace>,
    ) -> Result<(), StoreError> {
        use schema::agent::dsl;
        let _namespace = ns.get(namespaceid).ok_or(StoreError::InvalidNamespace {})?;
        let (_, nsid) = self.namespace_by_name(connection, namespaceid.decompose().0)?;

        diesel::insert_or_ignore_into(schema::agent::table)
            .values((
                dsl::name.eq(name),
                dsl::namespace_id.eq(nsid),
                dsl::current.eq(0),
                dsl::domaintype.eq(domaintypeid.as_ref().map(|x| x.decompose())),
            ))
            .execute(connection)?;

        Ok(())
    }

    #[instrument(skip(connection))]
    fn apply_attachment(
        &self,
        connection: &mut SqliteConnection,
        Attachment {
            namespaceid,
            signature,
            signer,
            locator,
            signature_time,
            ..
        }: &Attachment,
        ns: &HashMap<NamespaceId, Namespace>,
    ) -> Result<(), StoreError> {
        let _namespace = ns.get(namespaceid).ok_or(StoreError::InvalidNamespace {})?;
        let (_, nsid) = self.namespace_by_name(connection, namespaceid.decompose().0)?;
        let (agent_name, public_key) = signer.decompose();

        use schema::agent::dsl as agentdsl;
        use schema::identity::dsl as identitydsl;
        let signer_id = agentdsl::agent
            .inner_join(identitydsl::identity)
            .filter(
                agentdsl::name
                    .eq(agent_name)
                    .and(agentdsl::namespace_id.eq(nsid))
                    .and(identitydsl::public_key.eq(public_key)),
            )
            .select(identitydsl::id)
            .first::<i32>(connection)?;

        use schema::attachment::dsl;

        diesel::insert_or_ignore_into(schema::attachment::table)
            .values((
                dsl::namespace_id.eq(nsid),
                dsl::signature.eq(signature),
                dsl::signer_id.eq(signer_id),
                dsl::locator.eq(locator),
                dsl::signature_time.eq(Utc::now().naive_utc()),
            ))
            .execute(connection)?;

        Ok(())
    }

    #[instrument(skip(connection))]
    fn apply_entity(
        &self,
        connection: &mut SqliteConnection,
        entity: &common::prov::Entity,
        ns: &HashMap<NamespaceId, Namespace>,
    ) -> Result<(), StoreError> {
        use schema::entity::dsl;
        let (_namespaceid, nsid) =
            self.namespace_by_name(connection, entity.namespaceid.decompose().0)?;

        diesel::insert_or_ignore_into(schema::entity::table)
            .values((
                dsl::name.eq(&*entity.name),
                dsl::namespace_id.eq(nsid),
                dsl::domaintype.eq(entity.domaintypeid.as_ref().map(|x| x.decompose())),
            ))
            .execute(connection)?;

        Ok(())
    }

    #[instrument(skip(connection))]
    fn apply_has_attachment(
        &self,
        connection: &mut SqliteConnection,
        model: &ProvModel,
        namespaceid: &NamespaceId,
        entity: &EntityId,
        attachment: &AttachmentId,
    ) -> Result<(), StoreError> {
        let (_, nsid) = self.namespace_by_name(connection, namespaceid.decompose().0)?;
        let attachment = self.attachment_by(connection, namespaceid, attachment)?;
        use schema::entity::dsl;

        diesel::update(schema::entity::table)
            .filter(
                dsl::name
                    .eq(entity.decompose())
                    .and(dsl::namespace_id.eq(nsid)),
            )
            .set(dsl::attachment_id.eq(attachment.id))
            .execute(connection)?;

        Ok(())
    }

    #[instrument(skip(connection))]
    fn apply_had_attachment(
        &self,
        connection: &mut SqliteConnection,
        model: &ProvModel,
        namespaceid: &NamespaceId,
        entity: &EntityId,
        attachment: &AttachmentId,
    ) -> Result<(), StoreError> {
        let attachment = self.attachment_by(connection, namespaceid, attachment)?;
        let entity =
            self.entity_by_entity_name_and_namespace(connection, entity.decompose(), namespaceid)?;
        use schema::hadattachment::dsl;

        diesel::insert_or_ignore_into(schema::hadattachment::table)
            .values((
                dsl::entity_id.eq(entity.id),
                dsl::attachment_id.eq(attachment.id),
            ))
            .execute(connection)?;

        Ok(())
    }

    #[instrument(skip(connection))]
    fn apply_has_identity(
        &self,
        connection: &mut SqliteConnection,
        model: &ProvModel,
        namespaceid: &NamespaceId,
        agent: &AgentId,
        identity: &IdentityId,
    ) -> Result<(), StoreError> {
        let (_, nsid) = self.namespace_by_name(connection, namespaceid.decompose().0)?;
        let identity = self.identity_by(connection, namespaceid, identity)?;
        use schema::agent::dsl;

        diesel::update(schema::agent::table)
            .filter(
                dsl::name
                    .eq(agent.decompose())
                    .and(dsl::namespace_id.eq(nsid)),
            )
            .set(dsl::identity_id.eq(identity.id))
            .execute(connection)?;

        Ok(())
    }

    #[instrument(skip(connection))]
    fn apply_had_identity(
        &self,
        connection: &mut SqliteConnection,
        model: &ProvModel,
        namespaceid: &NamespaceId,
        agent: &AgentId,
        identity: &IdentityId,
    ) -> Result<(), StoreError> {
        let identity = self.identity_by(connection, namespaceid, identity)?;
        let agent =
            self.agent_by_agent_name_and_namespace(connection, agent.decompose(), namespaceid)?;
        use schema::hadidentity::dsl;

        diesel::insert_or_ignore_into(schema::hadidentity::table)
            .values((dsl::agent_id.eq(agent.id), dsl::identity_id.eq(identity.id)))
            .execute(connection)?;

        Ok(())
    }

    #[instrument(skip(connection))]
    fn apply_identity(
        &self,
        connection: &mut SqliteConnection,
        Identity {
            id,
            namespaceid,
            public_key,
            ..
        }: &Identity,
        ns: &HashMap<NamespaceId, Namespace>,
    ) -> Result<(), StoreError> {
        use schema::identity::dsl;
        let _namespace = ns.get(namespaceid).ok_or(StoreError::InvalidNamespace {})?;
        let (_, nsid) = self.namespace_by_name(connection, namespaceid.decompose().0)?;

        diesel::insert_or_ignore_into(schema::identity::table)
            .values((dsl::namespace_id.eq(nsid), dsl::public_key.eq(public_key)))
            .execute(connection)?;

        Ok(())
    }

    #[instrument(skip(connection))]
    fn apply_model(
        &self,
        connection: &mut SqliteConnection,
        model: &ProvModel,
    ) -> Result<(), StoreError> {
        for (_, ns) in model.namespaces.iter() {
            self.apply_namespace(connection, ns)?
        }
        for (_, agent) in model.agents.iter() {
            self.apply_agent(connection, agent, &model.namespaces)?
        }
        for (_, activity) in model.activities.iter() {
            self.apply_activity(connection, activity, &model.namespaces)?
        }
        for (_, entity) in model.entities.iter() {
            self.apply_entity(connection, entity, &model.namespaces)?
        }
        for (_, identity) in model.identities.iter() {
            self.apply_identity(connection, identity, &model.namespaces)?
        }
        for (_, attachment) in model.attachments.iter() {
            self.apply_attachment(connection, attachment, &model.namespaces)?
        }

        for ((namespaceid, agent_id), (_, identity_id)) in model.has_identity.iter() {
            debug!(
                namespace = ?namespaceid,
                agent_id = ?agent_id,
                identity_id = ?identity_id,
                "Apply has identity"
            );
            self.apply_has_identity(connection, model, namespaceid, agent_id, identity_id)?;
        }

        for ((namespaceid, agent_id), identity_id) in model.had_identity.iter() {
            debug!(
                namespace = ?namespaceid,
                agent_id = ?agent_id,
                identity_id = ?identity_id,
                "Apply had identity"
            );
            for (_, identity_id) in identity_id {
                self.apply_had_identity(connection, model, namespaceid, agent_id, identity_id)?;
            }
        }

        for ((namespaceid, entity_id), (_, attachment_id)) in model.has_attachment.iter() {
            debug!(
                namespace = ?namespaceid,
                entity_id = ?entity_id,
                attachment_id = ?attachment_id,
                "Apply has attachment"
            );
            self.apply_has_attachment(connection, model, namespaceid, entity_id, attachment_id)?;
        }

        for ((namespaceid, entity_id), attachment_id) in model.had_attachment.iter() {
            debug!(
                namespace = ?namespaceid,
                entity_id = ?entity_id,
                attachment_id = ?attachment_id,
                "Apply had attachment"
            );
            for (_, attachment_id) in attachment_id {
                self.apply_had_attachment(
                    connection,
                    model,
                    namespaceid,
                    entity_id,
                    attachment_id,
                )?;
            }
        }

        for ((namespaceid, activityid), agentid) in model.was_associated_with.iter() {
            debug!(
                namespace = ?namespaceid,
                activity = ?activityid,
                agent = ?agentid,
                "Apply was associated with"
            );
            for (_, agentid) in agentid {
                self.apply_was_associated_with(
                    connection,
                    model,
                    namespaceid,
                    activityid,
                    agentid,
                )?;
            }
        }

        for ((namespaceid, activityid), entityid) in model.used.iter() {
            for (_, entityid) in entityid {
                debug!(
                    namespace = ?namespaceid,
                    activity = ?activityid,
                    entity = ?entityid,
                    "Apply used"
                );
                self.apply_used(connection, model, namespaceid, activityid, entityid)?;
            }
        }

        for ((namespaceid, entityid), activityid) in model.was_generated_by.iter() {
            for (_, activityid) in activityid {
                debug!(
                    namespace = ?namespaceid,
                    activity = ?activityid,
                    entity = ?entityid,
                    "Apply was generated by"
                );
                self.apply_was_generated_by(connection, model, namespaceid, entityid, activityid)?;
            }
        }

        Ok(())
    }

    #[instrument(skip(connection))]
    fn apply_namespace(
        &self,
        connection: &mut SqliteConnection,
        Namespace {
            ref name, ref uuid, ..
        }: &Namespace,
    ) -> Result<(), StoreError> {
        use schema::namespace::dsl;
        diesel::insert_or_ignore_into(schema::namespace::table)
            .values((dsl::name.eq(name), dsl::uuid.eq(uuid.to_string())))
            .execute(connection)?;

        Ok(())
    }

    #[instrument]
    pub fn apply_prov(&self, prov: &ProvModel) -> Result<(), StoreError> {
        debug!("Enter transaction");

        trace!(?prov);
        self.connection()?.immediate_transaction(|connection| {
            debug!("Entered transaction");
            self.apply_model(connection, prov)
        })?;
        debug!("Completed transaction");

        Ok(())
    }

    #[instrument(skip(connection))]
    fn apply_used(
        &self,
        connection: &mut SqliteConnection,
        model: &ProvModel,
        namespace: &NamespaceId,
        activity: &ActivityId,
        entity: &common::prov::EntityId,
    ) -> Result<(), StoreError> {
        let proventity = model
            .entities
            .get(&(namespace.to_owned(), entity.to_owned()))
            .ok_or_else(|| StoreError::ModelDoesNotContainEntity {
                entityid: entity.clone(),
            })?;
        let provactivity = model
            .activities
            .get(&(namespace.to_owned(), activity.to_owned()))
            .ok_or_else(|| StoreError::ModelDoesNotContainActivity {
                activityid: activity.clone(),
            })?;

        let storedactivity = self.activity_by_activity_name_and_namespace(
            connection,
            &provactivity.name,
            &provactivity.namespaceid,
        )?;

        let storedentity = self.entity_by_entity_name_and_namespace(
            connection,
            &proventity.name,
            &proventity.namespaceid,
        )?;

        use schema::used::dsl as link;
        diesel::insert_or_ignore_into(schema::used::table)
            .values((
                &link::activity_id.eq(storedactivity.id),
                &link::entity_id.eq(storedentity.id),
            ))
            .execute(connection)?;

        Ok(())
    }

    #[instrument(skip(connection))]
    fn apply_was_associated_with(
        &self,
        connection: &mut SqliteConnection,
        model: &ProvModel,
        namespaceid: &common::prov::NamespaceId,
        activityid: &common::prov::ActivityId,
        agentid: &common::prov::AgentId,
    ) -> Result<(), StoreError> {
        let provagent = model
            .agents
            .get(&(namespaceid.to_owned(), agentid.to_owned()))
            .ok_or_else(|| StoreError::ModelDoesNotContainAgent {
                agentid: agentid.clone(),
            })?;
        let provactivity = model
            .activities
            .get(&(namespaceid.to_owned(), activityid.to_owned()))
            .ok_or_else(|| StoreError::ModelDoesNotContainActivity {
                activityid: activityid.clone(),
            })?;

        let storedactivity = self.activity_by_activity_name_and_namespace(
            connection,
            &provactivity.name,
            &provactivity.namespaceid,
        )?;

        let storedagent = self.agent_by_agent_name_and_namespace(
            connection,
            &provagent.name,
            &provagent.namespaceid,
        )?;

        use schema::wasassociatedwith::dsl as link;
        diesel::insert_or_ignore_into(schema::wasassociatedwith::table)
            .values((
                &link::activity_id.eq(storedactivity.id),
                &link::agent_id.eq(storedagent.id),
            ))
            .execute(connection)?;

        Ok(())
    }

    #[instrument(skip(connection))]
    fn apply_was_generated_by(
        &self,
        connection: &mut SqliteConnection,
        model: &ProvModel,
        namespace: &common::prov::NamespaceId,
        entity: &common::prov::EntityId,
        activity: &ActivityId,
    ) -> Result<(), StoreError> {
        let proventity = model
            .entities
            .get(&(namespace.to_owned(), entity.to_owned()))
            .ok_or_else(|| StoreError::ModelDoesNotContainEntity {
                entityid: entity.clone(),
            })?;
        let provactivity = model
            .activities
            .get(&(namespace.to_owned(), activity.to_owned()))
            .ok_or_else(|| StoreError::ModelDoesNotContainActivity {
                activityid: activity.clone(),
            })?;

        let storedactivity = self.activity_by_activity_name_and_namespace(
            connection,
            &provactivity.name,
            &provactivity.namespaceid,
        )?;

        let storedentity = self.entity_by_entity_name_and_namespace(
            connection,
            &proventity.name,
            &proventity.namespaceid,
        )?;

        use schema::wasgeneratedby::dsl as link;
        diesel::insert_or_ignore_into(schema::wasgeneratedby::table)
            .values((
                &link::activity_id.eq(storedactivity.id),
                &link::entity_id.eq(storedentity.id),
            ))
            .execute(connection)?;

        Ok(())
    }

    pub fn connection(
        &self,
    ) -> Result<PooledConnection<ConnectionManager<SqliteConnection>>, StoreError> {
        Ok(self.pool.get()?)
    }

    /// Ensure the name is unique within the namespace, if not, then postfix the rowid
    pub(crate) fn disambiguate_activity_name(
        &self,
        connection: &mut SqliteConnection,
        name: &str,
        namespaceid: &NamespaceId,
    ) -> Result<String, StoreError> {
        use schema::activity::dsl;
        use schema::namespace::dsl as nsdsl;

        let collision = schema::activity::table
            .inner_join(schema::namespace::table)
            .filter(
                dsl::name
                    .eq(name)
                    .and(nsdsl::name.eq(namespaceid.decompose().0)),
            )
            .count()
            .first::<i64>(connection)?;

        if collision == 0 {
            return Ok(name.to_owned());
        }

        let ambiguous = schema::activity::table
            .select(max(dsl::id))
            .first::<Option<i32>>(connection)?;

        Ok(format!("{}-{}", name, ambiguous.unwrap_or_default()))
    }

    /// Ensure the name is unique within the namespace, if not, then postfix the rowid
    pub(crate) fn disambiguate_agent_name(
        &self,
        connection: &mut SqliteConnection,
        name: &str,
        namespaceid: &NamespaceId,
    ) -> Result<String, StoreError> {
        use schema::agent::dsl;
        use schema::namespace::dsl as nsdsl;

        let collision = schema::agent::table
            .inner_join(schema::namespace::table)
            .filter(
                dsl::name
                    .eq(name)
                    .and(nsdsl::name.eq(namespaceid.decompose().0)),
            )
            .count()
            .first::<i64>(connection)?;

        if collision == 0 {
            return Ok(name.to_owned());
        }

        let ambiguous = schema::agent::table
            .select(max(dsl::id))
            .first::<Option<i32>>(connection)?;

        Ok(format!("{}-{}", name, ambiguous.unwrap_or_default()))
    }

    /// Ensure the name is unique within the namespace, if not, then postfix the rowid
    #[instrument(skip(connection))]
    pub(crate) fn disambiguate_entity_name(
        &self,
        connection: &mut SqliteConnection,
        name: &str,
        namespaceid: &NamespaceId,
    ) -> Result<String, StoreError> {
        use schema::entity::dsl;
        use schema::namespace::dsl as nsdsl;

        let collision = schema::entity::table
            .inner_join(schema::namespace::table)
            .filter(
                dsl::name
                    .eq(name)
                    .and(nsdsl::name.eq(namespaceid.decompose().0)),
            )
            .count()
            .first::<i64>(connection)?;

        if collision == 0 {
            trace!(
                ?name,
                "Entity name is unique within namespace, so use directly"
            );
            return Ok(name.to_owned());
        }

        let ambiguous = schema::entity::table
            .select(max(dsl::id))
            .first::<Option<i32>>(connection)?;

        trace!(?name, "Is not unique, postfix with last rowid");

        Ok(format!("{}-{}", name, ambiguous.unwrap_or_default()))
    }

    pub(crate) fn entity_by_entity_name_and_namespace(
        &self,
        connection: &mut SqliteConnection,
        name: &str,
        namespaceid: &NamespaceId,
    ) -> Result<query::Entity, StoreError> {
        let (_namespaceid, nsid) = self.namespace_by_name(connection, namespaceid.decompose().0)?;
        use schema::entity::dsl;

        Ok(schema::entity::table
            .filter(dsl::name.eq(name).and(dsl::namespace_id.eq(nsid)))
            .first::<query::Entity>(connection)?)
    }

    /// Get the named acitvity or the last started one, a useful context aware shortcut for the CLI
    #[instrument(skip(connection))]
    pub(crate) fn get_activity_by_name_or_last_started(
        &self,
        connection: &mut SqliteConnection,
        name: Option<String>,
        namespace: &NamespaceId,
    ) -> Result<query::Activity, StoreError> {
        use schema::activity::dsl;

        if let Some(name) = name {
            trace!(%name, "Use existing");
            Ok(self.activity_by_activity_name_and_namespace(connection, &name, namespace)?)
        } else {
            trace!("Use last started");
            Ok(schema::activity::table
                .order(dsl::started)
                .first::<query::Activity>(connection)?)
        }
    }

    #[instrument(skip(connection))]
    pub(crate) fn get_current_agent(
        &self,
        connection: &mut SqliteConnection,
    ) -> Result<query::Agent, StoreError> {
        use schema::agent::dsl;
        Ok(schema::agent::table
            .filter(dsl::current.ne(0))
            .first::<query::Agent>(connection)?)
    }

    /// Get the last fully syncronised offset
    #[instrument]
    pub fn get_last_offset(&self) -> Result<Option<Offset>, StoreError> {
        use schema::ledgersync::dsl;
        Ok(self.connection()?.immediate_transaction(|connection| {
            schema::ledgersync::table
                .order_by(dsl::sync_time)
                .select(dsl::offset)
                .first::<String>(connection)
                .optional()
                .map(|sync| sync.map(|sync| Offset::from(&*sync)))
        })?)
    }

    #[instrument(skip(connection))]
    pub(crate) fn namespace_by_name(
        &self,
        connection: &mut SqliteConnection,
        namespace: &str,
    ) -> Result<(NamespaceId, i32), StoreError> {
        use self::schema::namespace::dsl;

        let ns = dsl::namespace
            .filter(dsl::name.eq(namespace))
            .select((dsl::id, dsl::name, dsl::uuid))
            .first::<(i32, String, String)>(connection)
            .optional()?
            .ok_or(StoreError::RecordNotFound {})?;

        Ok((
            Chronicle::namespace(&ns.1, &Uuid::from_str(&ns.2)?).into(),
            ns.0,
        ))
    }

    #[instrument(skip(connection))]
    pub(crate) fn attachment_by(
        &self,
        connection: &mut SqliteConnection,
        namespaceid: &NamespaceId,
        attachment: &AttachmentId,
    ) -> Result<query::Attachment, StoreError> {
        use self::schema::attachment::dsl;
        let (_, nsid) = self.namespace_by_name(connection, namespaceid.decompose().0)?;
        let (_entity_name, id_signature) = attachment.decompose();

        Ok(dsl::attachment
            .filter(
                dsl::signature
                    .eq(id_signature)
                    .and(dsl::namespace_id.eq(nsid)),
            )
            .first::<query::Attachment>(connection)?)
    }

    #[instrument(skip(connection))]
    pub(crate) fn identity_by(
        &self,
        connection: &mut SqliteConnection,
        namespaceid: &NamespaceId,
        identity: &IdentityId,
    ) -> Result<query::Identity, StoreError> {
        use self::schema::identity::dsl;
        let (_, nsid) = self.namespace_by_name(connection, namespaceid.decompose().0)?;
        let (_agent_name, public_key) = identity.decompose();

        Ok(dsl::identity
            .filter(
                dsl::public_key
                    .eq(public_key)
                    .and(dsl::namespace_id.eq(nsid)),
            )
            .first::<query::Identity>(connection)?)
    }

    #[instrument]
    pub fn new(pool: Pool<ConnectionManager<SqliteConnection>>) -> Result<Self, StoreError> {
        Ok(Store { pool })
    }

    #[instrument(skip(connection))]
    pub fn prov_model_for_namespace(
        &self,
        connection: &mut SqliteConnection,
        query: QueryCommand,
    ) -> Result<ProvModel, StoreError> {
        let mut model = ProvModel::default();
        let (namespaceid, nsid) = self.namespace_by_name(connection, &query.namespace)?;

        let agents = schema::agent::table
            .filter(schema::agent::namespace_id.eq(&nsid))
            .load::<query::Agent>(connection)?;

        for agent in agents {
            debug!(?agent, "Map agent to prov");
            let agentid: AgentId = Chronicle::agent(&agent.name).into();
            model.agents.insert(
                (namespaceid.clone(), agentid.clone()),
                Agent {
                    id: agentid.clone(),
                    namespaceid: namespaceid.clone(),
                    name: agent.name,
                    domaintypeid: agent.domaintype.map(|x| Chronicle::domaintype(&x).into()),
                },
            );

            for asoc in schema::wasassociatedwith::table
                .filter(schema::wasassociatedwith::agent_id.eq(agent.id))
                .inner_join(schema::activity::table)
                .select(schema::activity::name)
                .load_iter::<String>(connection)?
            {
                let asoc = asoc?;
                model.associate_with(&namespaceid, &Chronicle::activity(&asoc).into(), &agentid);
            }
        }

        let activities = schema::activity::table
            .filter(schema::activity::namespace_id.eq(nsid))
            .load::<query::Activity>(connection)?;

        for activity in activities {
            debug!(?activity, "Map activity to prov");

            let id: ActivityId = Chronicle::activity(&activity.name).into();
            model.activities.insert(
                (namespaceid.clone(), id.clone()),
                Activity {
                    id: id.clone(),
                    namespaceid: namespaceid.clone(),
                    name: activity.name,
                    started: activity.started.map(|x| DateTime::from_utc(x, Utc)),
                    ended: activity.ended.map(|x| DateTime::from_utc(x, Utc)),
                    domaintypeid: activity
                        .domaintype
                        .map(|x| Chronicle::domaintype(&x).into()),
                },
            );

            for asoc in schema::wasgeneratedby::table
                .filter(schema::wasgeneratedby::activity_id.eq(activity.id))
                .inner_join(schema::entity::table)
                .select(schema::entity::name)
                .load_iter::<String>(connection)?
            {
                let asoc = asoc?;
                model.generate_by(namespaceid.clone(), &Chronicle::entity(&asoc).into(), &id);
            }

            for used in schema::used::table
                .filter(schema::used::activity_id.eq(activity.id))
                .inner_join(schema::entity::table)
                .select(schema::entity::name)
                .load_iter::<String>(connection)?
            {
                let used = used?;
                model.used(namespaceid.clone(), &id, &Chronicle::entity(&used).into());
            }
        }

        let entites = schema::entity::table
            .filter(schema::entity::namespace_id.eq(nsid))
            .load::<query::Entity>(connection)?;

        for query::Entity {
            id: _,
            namespace_id: _,
            domaintype,
            name,
            attachment_id: _,
        } in entites
        {
            let id: EntityId = Chronicle::entity(&name).into();
            model.entities.insert(
                (namespaceid.clone(), id.clone()),
                Entity {
                    id,
                    namespaceid: namespaceid.clone(),
                    name,
                    domaintypeid: domaintype.map(|x| Chronicle::domaintype(&x).into()),
                },
            );
        }

        Ok(model)
    }

    /// Set the last fully syncronised offset
    #[instrument]
    pub fn set_last_offset(&self, offset: Offset) -> Result<(), StoreError> {
        use schema::ledgersync::{self as dsl};

        if let Offset::Identity(offset) = offset {
            Ok(self.connection()?.immediate_transaction(|connection| {
                diesel::insert_into(dsl::table)
                    .values((
                        dsl::offset.eq(offset),
                        (dsl::sync_time.eq(Utc::now().naive_utc())),
                    ))
                    .on_conflict(dsl::offset)
                    .do_update()
                    .set(dsl::sync_time.eq(Utc::now().naive_utc()))
                    .execute(connection)
                    .map(|_| ())
            })?)
        } else {
            Ok(())
        }
    }

    #[instrument(skip(connection))]
    pub(crate) fn use_agent(
        &self,
        connection: &mut SqliteConnection,
        name: String,
        namespace: String,
    ) -> Result<(), StoreError> {
        let (_, nsid) = self.namespace_by_name(connection, &*namespace)?;
        use schema::agent::dsl;

        diesel::update(schema::agent::table.filter(dsl::current.ne(0)))
            .set(dsl::current.eq(0))
            .execute(connection)?;

        diesel::update(
            schema::agent::table.filter(dsl::name.eq(name).and(dsl::namespace_id.eq(nsid))),
        )
        .set(dsl::current.eq(1))
        .execute(connection)?;

        Ok(())
    }
}
