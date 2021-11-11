use async_std::task::block_on;
use chrono::{DateTime, Utc};
use iref::{AsIri, Iri};
use json::{object, JsonValue};
use json_ld::{context::Local, Document, JsonContext, NoLoader};
use serde::Serialize;

use std::{
    collections::{HashMap, HashSet},
};
use uuid::Uuid;

use crate::vocab::{Chronicle, Prov};

#[derive(Serialize, Deserialize, PartialEq, Eq, Hash, Debug, Clone)]
pub struct NamespaceId(String);

impl std::ops::Deref for NamespaceId {
    type Target = String;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl NamespaceId {
    pub fn new<S>(s: S) -> Self
    where
        S: AsRef<str>,
    {
        NamespaceId(s.as_ref().to_owned())
    }
    /// Decompose a namespace id into its constituent parts, we need to preserve the type better to justify this implementation
    pub fn decompose(&self) -> (&str, Uuid) {
        if let &[_, _, name, uuid, ..] = &self.0.split(':').collect::<Vec<_>>()[..] {
            return (name, Uuid::parse_str(uuid).unwrap());
        }

        unreachable!();
    }
}

impl<S> From<S> for NamespaceId
where
    S: AsIri,
{
    fn from(iri: S) -> Self {
        Self(iri.as_iri().to_string())
    }
}

impl Into<String> for NamespaceId {
    fn into(self) -> String {
        self.0
    }
}

#[derive(Serialize, Deserialize, PartialEq, Eq, Hash, Debug, Clone)]
pub struct EntityId(String);

impl std::ops::Deref for EntityId {
    type Target = String;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl<S> From<S> for EntityId
where
    S: AsIri,
{
    fn from(iri: S) -> Self {
        Self(iri.as_iri().to_string())
    }
}

impl Into<String> for EntityId {
    fn into(self) -> String {
        self.0
    }
}

impl EntityId {
    pub fn new<S>(s: S) -> Self
    where
        S: AsRef<str>,
    {
        Self(s.as_ref().to_owned())
    }
    /// Extract the activity name from an id
    pub fn decompose(&self) -> &str {
        if let &[_, _, name, ..] = &self.0.split(':').collect::<Vec<_>>()[..] {
            return name;
        }

        unreachable!();
    }
}

#[derive(Serialize, Deserialize, PartialEq, Eq, Hash, Debug, Clone)]
pub struct AgentId(String);

impl Into<String> for AgentId {
    fn into(self) -> String {
        self.0
    }
}

impl std::ops::Deref for AgentId {
    type Target = String;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl AgentId {
    pub fn new<S>(s: S) -> Self
    where
        S: AsRef<str>,
    {
        Self(s.as_ref().to_owned())
    }
    /// Extract the agent name from an id
    pub fn decompose(&self) -> &str {
        if let &[_, _, name, ..] = &self.0.split(':').collect::<Vec<_>>()[..] {
            return name;
        }

        unreachable!();
    }
}

impl<S> From<S> for AgentId
where
    S: AsIri,
{
    fn from(iri: S) -> Self {
        Self(iri.as_iri().to_string())
    }
}

#[derive(Serialize, Deserialize, PartialEq, Eq, Hash, Debug, Clone)]
pub struct ActivityId(String);

impl Into<String> for ActivityId {
    fn into(self) -> String {
        self.0
    }
}

impl std::ops::Deref for ActivityId {
    type Target = String;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl ActivityId {
    pub fn new<S>(s: S) -> Self
    where
        S: AsRef<str>,
    {
        Self(s.as_ref().to_owned())
    }
    /// Extract the activity name from an id
    pub fn decompose(&self) -> &str {
        if let &[_, _, name, ..] = &self.0.split(':').collect::<Vec<_>>()[..] {
            return name;
        }

        unreachable!();
    }
}

impl<S> From<S> for ActivityId
where
    S: AsIri,
{
    fn from(iri: S) -> Self {
        Self(iri.as_iri().to_string())
    }
}

#[derive(Serialize, Deserialize, PartialEq, Debug, Clone)]
pub struct CreateNamespace {
    pub id: NamespaceId,
    pub name: String,
    pub uuid: Uuid,
}

#[derive(Serialize, Deserialize, PartialEq, Debug, Clone)]
pub struct CreateAgent {
    pub namespace: NamespaceId,
    pub name: String,
    pub id: AgentId,
}

#[derive(Serialize, Deserialize, PartialEq, Debug, Clone)]
pub struct RegisterKey {
    pub namespace: NamespaceId,
    pub id: AgentId,
    pub publickey: String,
    pub name: String,
}

#[derive(Serialize, Deserialize, PartialEq, Debug, Clone)]
pub struct CreateActivity {
    pub namespace: NamespaceId,
    pub id: ActivityId,
    pub name: String,
}

#[derive(Serialize, Deserialize, PartialEq, Debug, Clone)]
pub struct StartActivity {
    pub namespace: NamespaceId,
    pub id: ActivityId,
    pub agent: AgentId,
    pub time: DateTime<Utc>,
}

#[derive(Serialize, Deserialize, PartialEq, Debug, Clone)]
pub struct EndActivity {
    pub namespace: NamespaceId,
    pub id: ActivityId,
    pub agent: AgentId,
    pub time: DateTime<Utc>,
}

#[derive(Serialize, Deserialize, PartialEq, Debug, Clone)]
pub struct ActivityUses {
    pub namespace: NamespaceId,
    pub id: EntityId,
    pub activity: ActivityId,
}

#[derive(Serialize, Deserialize, PartialEq, Debug, Clone)]
pub struct GenerateEntity {
    pub namespace: NamespaceId,
    pub id: EntityId,
    pub activity: ActivityId,
}

#[derive(Serialize, Deserialize, PartialEq, Debug, Clone)]
pub struct EntityAttach {
    pub namespace: NamespaceId,
    pub id: EntityId,
    pub agent: AgentId,
    pub signature: String,
    pub locator: Option<String>,
    pub signature_time: DateTime<Utc>,
}

#[derive(Serialize, Deserialize, PartialEq, Debug, Clone)]
pub enum ChronicleTransaction {
    CreateNamespace(CreateNamespace),
    CreateAgent(CreateAgent),
    RegisterKey(RegisterKey),
    CreateActivity(CreateActivity),
    StartActivity(StartActivity),
    EndActivity(EndActivity),
    ActivityUses(ActivityUses),
    GenerateEntity(GenerateEntity),
    EntityAttach(EntityAttach),
}

#[derive(Debug, Clone)]
pub struct Namespace {
    pub id: NamespaceId,
    pub uuid: Uuid,
    pub name: String,
}

impl Namespace {
    pub fn new(id: NamespaceId, uuid: Uuid, name: String) -> Self {
        Self { id, uuid, name }
    }
}

#[derive(Debug, Clone)]
pub struct Agent {
    pub id: AgentId,
    pub namespaceid: NamespaceId,
    pub name: String,
    pub publickey: Option<String>,
}

impl Agent {
    pub fn new(
        id: AgentId,
        namespaceid: NamespaceId,
        name: String,
        publickey: Option<String>,
    ) -> Self {
        Self {
            id,
            namespaceid,
            name,
            publickey,
        }
    }
}

#[derive(Debug, Clone)]
pub struct Activity {
    pub id: ActivityId,
    pub namespaceid: NamespaceId,
    pub name: String,
    pub started: Option<DateTime<Utc>>,
    pub ended: Option<DateTime<Utc>>,
}

impl Activity {
    pub fn new(id: ActivityId, ns: NamespaceId, name: &str) -> Self {
        Self {
            id,
            namespaceid: ns,
            name: name.to_owned(),
            started: None,
            ended: None,
        }
    }
}

#[derive(Debug, Clone)]
pub enum Entity {
    Unsigned {
        id: EntityId,
        namespaceid: NamespaceId,
        name: String,
    },
    Signed {
        id: EntityId,
        namespaceid: NamespaceId,
        name: String,
        signature: String,
        locator: Option<String>,
        signature_time: DateTime<Utc>,
    },
}

impl Entity {
    pub fn unsigned(id: EntityId, namespaceid: &NamespaceId, name: &str) -> Self {
        Self::Unsigned {
            id,
            namespaceid: namespaceid.to_owned(),
            name: name.to_owned(),
        }
    }

    pub fn name(&self) -> &str {
        match self {
            Self::Unsigned { name, .. } | Self::Signed { name, .. } => name,
        }
    }

    pub fn id(&self) -> &EntityId {
        match self {
            Self::Unsigned { id, .. } | Self::Signed { id, .. } => id,
        }
    }
    pub fn namespaceid(&self) -> &NamespaceId {
        match self {
            Self::Unsigned { namespaceid, .. } | Self::Signed { namespaceid, .. } => namespaceid,
        }
    }

    pub fn sign(
        self,
        signature: String,
        locator: Option<String>,
        signature_time: DateTime<Utc>,
    ) -> Self {
        match self {
            Self::Unsigned {
                id,
                namespaceid,
                name,
            }
            | Self::Signed {
                id,
                namespaceid,
                name,
                ..
            } => Self::Signed {
                id,
                namespaceid,
                name,
                signature,
                locator,
                signature_time,
            },
        }
    }
}

#[derive(Debug, Default)]
pub struct ProvModel {
    pub namespaces: HashMap<NamespaceId, Namespace>,
    pub agents: HashMap<AgentId, Agent>,
    pub activities: HashMap<ActivityId, Activity>,
    pub entities: HashMap<EntityId, Entity>,
    pub was_associated_with: HashMap<ActivityId, HashSet<AgentId>>,
    pub was_attributed_to: HashMap<EntityId, HashSet<AgentId>>,
    pub was_generated_by: HashMap<EntityId, HashSet<ActivityId>>,
    pub used: HashMap<ActivityId, HashSet<EntityId>>,
}

impl ProvModel {
    pub fn from_tx<'a, I>(tx: I) -> Self
    where
        I: IntoIterator<Item = &'a ChronicleTransaction>,
    {
        let mut model = Self::default();
        for tx in tx {
            model.apply(tx);
        }

        model
    }

    pub fn associate_with(&mut self, activity: ActivityId, agent: &AgentId) {
        self.was_associated_with
            .entry(activity)
            .or_insert(HashSet::new())
            .insert(agent.clone());
    }

    pub fn generate_by(&mut self, entity: EntityId, activity: &ActivityId) {
        self.was_generated_by
            .entry(entity)
            .or_insert(HashSet::new())
            .insert(activity.clone());
    }

    pub fn used(&mut self, activity: ActivityId, entity: &EntityId) {
        self.used
            .entry(activity)
            .or_insert(HashSet::new())
            .insert(entity.clone());
    }

    pub fn namespace_context(&mut self, ns: &NamespaceId) {
        let (namespacename, uuid) = ns.decompose();

        self.namespaces.insert(
            ns.clone(),
            Namespace {
                id: ns.clone(),
                uuid,
                name: namespacename.to_owned(),
            },
        );
    }

    /// Transform a sequence of ChronicleTransaction events into a provenance model,
    /// If a statement requires a subject or object that does not currently exist in the model, then we create it
    pub fn apply(&mut self, tx: &ChronicleTransaction) {
        let tx = tx.to_owned();
        match tx {
            ChronicleTransaction::CreateNamespace(CreateNamespace { id, name, uuid }) => {
                self.namespaces
                    .insert(id.clone(), Namespace::new(id, uuid, name));
            }
            ChronicleTransaction::CreateAgent(CreateAgent {
                namespace,
                id,
                name,
            }) => {
                self.namespace_context(&namespace);
                self.agents
                    .insert(id.clone(), Agent::new(id, namespace, name, None));
            }
            ChronicleTransaction::RegisterKey(RegisterKey {
                namespace,
                id,
                publickey,
                name,
            }) => {
                self.namespace_context(&namespace);

                if !self.agents.contains_key(&id) {
                    self.agents
                        .insert(id.clone(), Agent::new(id.clone(), namespace, name, None));
                }
                self.agents
                    .get_mut(&id)
                    .map(|x| x.publickey = Some(publickey));
            }
            ChronicleTransaction::CreateActivity(CreateActivity {
                namespace,
                id,
                name,
            }) => {
                self.namespace_context(&namespace);

                if !self.activities.contains_key(&id) {
                    self.activities
                        .insert(id.clone(), Activity::new(id, namespace, &name));
                }
            }
            ChronicleTransaction::StartActivity(StartActivity {
                namespace,
                id,
                agent,
                time,
            }) => {
                self.namespace_context(&namespace);
                if !self.activities.contains_key(&id) {
                    let activity_name = id.decompose();
                    let mut activity = Activity::new(id.clone(), namespace.clone(), activity_name);
                    activity.started = Some(time);
                    self.activities.insert(id.clone(), activity);
                }

                if !self.agents.contains_key(&agent) {
                    let agent_name = agent.decompose();
                    let agentmodel =
                        Agent::new(agent.clone(), namespace, agent_name.to_owned(), None);
                    self.agents.insert(agent.clone(), agentmodel);
                }

                self.was_associated_with
                    .entry(id)
                    .or_insert(HashSet::new())
                    .insert(agent);
            }
            ChronicleTransaction::EndActivity(EndActivity {
                namespace,
                id,
                agent,
                time,
            }) => {
                self.namespace_context(&namespace);
                if !self.activities.contains_key(&id) {
                    let activity_name = id.decompose();
                    let mut activity = Activity::new(id.clone(), namespace.clone(), activity_name);
                    activity.ended = Some(time);
                    self.activities.insert(id.clone(), activity);
                }

                if !self.agents.contains_key(&agent) {
                    let agent_name = agent.decompose();
                    let agentmodel =
                        Agent::new(agent.clone(), namespace, agent_name.to_owned(), None);
                    self.agents.insert(agent.clone(), agentmodel);
                }

                self.was_associated_with
                    .entry(id)
                    .or_insert(HashSet::new())
                    .insert(agent);
            }
            ChronicleTransaction::ActivityUses(ActivityUses {
                namespace,
                id,
                activity,
            }) => {
                self.namespace_context(&namespace);
                if !self.activities.contains_key(&activity) {
                    let activity_name = activity.decompose();
                    self.activities.insert(
                        activity.clone(),
                        Activity::new(activity.clone(), namespace.clone(), activity_name),
                    );
                }
                if !self.entities.contains_key(&id) {
                    let name = id.decompose();
                    self.entities
                        .insert(id.clone(), Entity::unsigned(id.clone(), &namespace, name));
                }

                self.used
                    .entry(activity)
                    .or_insert(HashSet::new())
                    .insert(id);
            }
            ChronicleTransaction::GenerateEntity(GenerateEntity {
                namespace,
                id,
                activity,
            }) => {
                self.namespace_context(&namespace);
                if !self.activities.contains_key(&activity) {
                    let activity_name = activity.decompose();
                    self.activities.insert(
                        activity.clone(),
                        Activity::new(activity.clone(), namespace.clone(), activity_name),
                    );
                }
                if !self.entities.contains_key(&id) {
                    let name = id.decompose();
                    self.entities
                        .insert(id.clone(), Entity::unsigned(id.clone(), &namespace, name));
                }

                self.was_generated_by
                    .entry(id)
                    .or_insert(HashSet::new())
                    .insert(activity);
            }
            ChronicleTransaction::EntityAttach(EntityAttach {
                namespace,
                id,
                agent,
                signature,
                locator,
                signature_time,
            }) => {
                self.namespace_context(&namespace);

                if !self.entities.contains_key(&id) {
                    let name = id.decompose();
                    self.entities
                        .insert(id.clone(), Entity::unsigned(id.clone(), &namespace, name));
                }

                if !self.agents.contains_key(&agent) {
                    let agent_name = agent.decompose();
                    let agentmodel =
                        Agent::new(agent.clone(), namespace, agent_name.to_owned(), None);
                    self.agents.insert(agent.clone(), agentmodel);
                }

                let unsigned = self.entities.remove(&id).unwrap();

                self.entities.insert(
                    id.clone(),
                    unsigned.sign(signature, locator, signature_time),
                );
            }
        };
    }

    /// Write the model out as a JSON-LD document in expanded form
    pub fn to_json(&self) -> ExpandedJson {
        let mut doc = json::Array::new();

        for (id, ns) in self.namespaces.iter() {
            doc.push(object! {
                "@id": (*id.as_str()),
                "@type": Iri::from(Chronicle::NamespaceType).as_str(),
                "http://www.w3.org/2000/01/rdf-schema#label": [{
                    "@value": ns.name.as_str(),
                }]
            })
        }

        for (id, agent) in self.agents.iter() {
            let mut agentdoc = object! {
                "@id": (*id.as_str()),
                "@type": Iri::from(Prov::Agent).as_str(),
                "http://www.w3.org/2000/01/rdf-schema#label": [{
                   "@value": agent.name.as_str(),
                }]
            };
            agent.publickey.as_ref().map(|publickey| {
                let mut values = json::Array::new();

                values.push(object! {
                    "@value": JsonValue::String(publickey.to_owned()),
                });

                agentdoc
                    .insert(Iri::from(Chronicle::HasPublicKey).as_str(), values)
                    .ok();
            });

            let mut values = json::Array::new();

            values.push(object! {
                "@id": JsonValue::String(agent.namespaceid.0.clone()),
            });

            agentdoc
                .insert(Iri::from(Chronicle::HasNamespace).as_str(), values)
                .ok();

            doc.push(agentdoc);
        }

        for (id, activity) in self.activities.iter() {
            let mut activitydoc = object! {
                "@id": (*id.as_str()),
                "@type": Iri::from(Prov::Activity).as_str(),
                "http://www.w3.org/2000/01/rdf-schema#label": [{
                   "@value": activity.name.as_str(),
                }]
            };

            activity.started.map(|time| {
                let mut values = json::Array::new();
                values.push(object! {"@value": time.to_rfc3339()});

                activitydoc
                    .insert("http://www.w3.org/ns/prov#startedAtTime", values)
                    .ok();
            });

            activity.ended.map(|time| {
                let mut values = json::Array::new();
                values.push(object! {"@value": time.to_rfc3339()});

                activitydoc
                    .insert("http://www.w3.org/ns/prov#endedAtTime", values)
                    .ok();
            });

            self.was_associated_with.get(id).map(|asoc| {
                let mut ids = json::Array::new();

                for id in asoc.iter() {
                    ids.push(object! {"@id": id.as_str()});
                }

                activitydoc
                    .insert(&Iri::from(Prov::WasAssociatedWith).to_string(), ids)
                    .ok();
            });

            self.used.get(id).map(|asoc| {
                let mut ids = json::Array::new();

                for id in asoc.iter() {
                    ids.push(object! {"@id": id.as_str()});
                }

                activitydoc
                    .insert(&Iri::from(Prov::Used).to_string(), ids)
                    .ok();
            });

            let mut values = json::Array::new();

            values.push(object! {
                "@id": JsonValue::String(activity.namespaceid.0.clone()),
            });

            activitydoc
                .insert(Iri::from(Chronicle::HasNamespace).as_str(), values)
                .ok();

            doc.push(activitydoc);
        }

        for (id, entity) in self.entities.iter() {
            let mut entitydoc = object! {
                "@id": (*id.as_str()),
                "@type": Iri::from(Prov::Entity).as_str(),
                "http://www.w3.org/2000/01/rdf-schema#label": [{
                   "@value": entity.name()
                }]
            };

            self.was_generated_by.get(id).map(|asoc| {
                let mut ids = json::Array::new();

                for id in asoc.iter() {
                    ids.push(object! {"@id": id.as_str()});
                }

                entitydoc
                    .insert(Iri::from(Prov::WasGeneratedBy).as_str(), ids)
                    .ok();
            });

            if let Entity::Signed {
                signature,
                signature_time,
                locator,
                ..
            } = entity
            {
                entitydoc
                    .insert(
                        Iri::from(Chronicle::Signature).as_str(),
                        signature.to_owned(),
                    )
                    .ok();

                entitydoc
                    .insert(
                        Iri::from(Chronicle::SignedAtTime).as_str(),
                        signature_time.to_rfc3339(),
                    )
                    .ok();

                if let Some(locator) = locator.as_ref() {
                    entitydoc
                        .insert(Iri::from(Chronicle::Locator).as_str(), locator.to_owned())
                        .ok();
                }
            }

            let mut values = json::Array::new();

            values.push(object! {
                "@id": JsonValue::String(entity.namespaceid().0.clone()),
            });

            entitydoc
                .insert(Iri::from(Chronicle::HasNamespace).as_str(), values)
                .ok();

            doc.push(entitydoc);
        }

        ExpandedJson(doc.into())
    }

    pub(crate) fn add_agent(&mut self, agent: Agent) {
        self.agents.insert(agent.id.clone(), agent);
    }

    pub(crate) fn add_activity(&mut self, activity: Activity) {
        self.activities.insert(activity.id.clone(), activity);
    }

    pub(crate) fn add_entity(&mut self, entity: Entity) {
        self.entities.insert(entity.id().clone(), entity);
    }
}

pub struct ExpandedJson(pub JsonValue);

impl ExpandedJson {
    pub fn compact(self) -> Result<CompactedJson, json_ld::Error> {
        let processed_context =
            block_on(crate::context::PROV.process::<JsonContext, _>(&mut NoLoader, None))?;

        let output = block_on(self.0.compact(&processed_context, &mut NoLoader))?;

        Ok(CompactedJson(output))
    }
}

pub struct CompactedJson(pub JsonValue);

impl std::ops::Deref for CompactedJson {
    type Target = JsonValue;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}
