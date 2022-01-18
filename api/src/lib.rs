mod graphql;
mod persistence;

use chrono::{DateTime, Utc};
use custom_error::*;
use derivative::*;

use diesel::{connection::SimpleConnection, r2d2::ConnectionManager, SqliteConnection};
use futures::{AsyncReadExt, Future};
use iref::IriBuf;
use k256::ecdsa::{signature::Signer, Signature};
use persistence::{ConnectionOptions, Store};
use r2d2::Pool;
use std::{
    convert::Infallible,
    net::{AddrParseError, SocketAddr},
    path::Path,
    sync::Arc,
    time::Duration,
};
use tokio::sync::mpsc::{self, error::SendError, Sender};

use common::{
    commands::*,
    ledger::{LedgerReader, LedgerWriter, SubmissionError},
    prov::{vocab::Chronicle as ChronicleVocab, Domaintype, ProvModel},
    prov::{
        ActivityUses, ChronicleTransaction, CreateActivity, CreateAgent, CreateNamespace,
        EndActivity, EntityAttach, GenerateEntity, RegisterKey, StartActivity,
    },
    signing::{DirectoryStoredKeys, SignerError},
};

use tracing::{debug, error, instrument, trace};

use user_error::UFE;
use uuid::Uuid;

pub use graphql::serve_graphql;

custom_error! {pub ApiError
    Store{source: persistence::StoreError}                      = "Storage",
    Iri{source: iref::Error}                                    = "Invalid IRI",
    JsonLD{message: String}                                     = "Json LD processing",
    Ledger{source: SubmissionError}                             = "Ledger error",
    Signing{source: SignerError}                                = "Signing",
    NoCurrentAgent{}                                            = "No agent is currently in use, please call agent use or supply an agent in your call",
    CannotFindAttachment{}                                      = "Cannot locate attachment file",
    ApiShutdownRx                                               = "Api shut down before reply",
    ApiShutdownTx{source: SendError<ApiSendWithReply>}          = "Api shut down before send",
    AddressParse{source: AddrParseError}                        = "Invalid socket address",
    ConnectionPool{source: r2d2::Error}                         = "Connection pool",
    FileUpload{source: std::io::Error}                          = "File upload",
}

/// Ugly but we need this until ! is stable https://github.com/rust-lang/rust/issues/64715
impl From<Infallible> for ApiError {
    fn from(_: Infallible) -> Self {
        unreachable!()
    }
}

impl UFE for ApiError {}

type ApiSendWithReply = (ApiCommand, Sender<Result<ApiResponse, ApiError>>);

#[derive(Derivative)]
#[derivative(Debug)]
pub struct Api<W, R>
where
    W: LedgerWriter,
    R: LedgerReader,
{
    tx: Sender<ApiSendWithReply>,
    #[derivative(Debug = "ignore")]
    keystore: DirectoryStoredKeys,
    #[derivative(Debug = "ignore")]
    ledger_writer: W,
    #[derivative(Debug = "ignore")]
    ledger_reader: R,
    #[derivative(Debug = "ignore")]
    store: persistence::Store,
    #[derivative(Debug = "ignore")]
    uuidsource: Box<dyn Fn() -> Uuid + Send + 'static>,
}

#[derive(Debug, Clone)]
/// A clonable api handle
pub struct ApiDispatch {
    tx: Sender<ApiSendWithReply>,
}

impl ApiDispatch {
    #[instrument]
    pub async fn dispatch(&self, command: ApiCommand) -> Result<ApiResponse, ApiError> {
        let (reply_tx, mut reply_rx) = mpsc::channel(1);
        trace!(?command, "Dispatch command to api");
        self.tx.clone().send((command, reply_tx)).await?;

        let reply = reply_rx.recv().await;

        if let Some(Err(ref error)) = reply {
            error!(?error, "Api dispatch");
        }

        reply.ok_or(ApiError::ApiShutdownRx {})?
    }
}

impl<W: LedgerWriter + 'static + Send, R: LedgerReader + 'static + Send> Api<W, R> {
    #[instrument(skip(ledger_writer, ledger_reader, uuidgen))]
    pub fn new<F>(
        addr: SocketAddr,
        dbpath: &str,
        ledger_writer: W,
        ledger_reader: R,
        secret_path: &Path,
        uuidgen: F,
    ) -> Result<(ApiDispatch, impl Future<Output = ()>), ApiError>
    where
        F: Fn() -> Uuid + Send + 'static,
    {
        let (tx, mut rx) = mpsc::channel::<ApiSendWithReply>(10);

        let pool = Pool::builder()
            .connection_customizer(Box::new(ConnectionOptions {
                enable_wal: false,
                enable_foreign_keys: true,
                busy_timeout: Some(Duration::from_secs(2)),
            }))
            .build(ConnectionManager::<SqliteConnection>::new(dbpath))?;

        let dispatch = ApiDispatch { tx: tx.clone() };
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();

        let secret_path = secret_path.to_owned();

        let store = Store::new(pool.clone())?;

        let ql_pool = pool;

        std::thread::spawn(move || {
            let local = tokio::task::LocalSet::new();
            local.spawn_local(async move {
                let keystore = DirectoryStoredKeys::new(secret_path).unwrap();

                let mut api = Api {
                    tx: tx.clone(),
                    keystore,
                    ledger_writer,
                    ledger_reader,
                    store,
                    uuidsource: Box::new(uuidgen),
                };

                debug!(?api, "Api running on localset");

                loop {
                    if let Some((command, reply)) = rx.recv().await {
                        trace!(?rx, "Recv api command from channel");

                        let result = api.dispatch(command).await;

                        reply
                            .send(result)
                            .await
                            .map_err(|e| {
                                error!(?e);
                            })
                            .ok();
                    } else {
                        return;
                    }
                }
            });

            rt.block_on(local)
        });

        Ok((
            dispatch.clone(),
            graphql::serve_graphql(ql_pool, dispatch, addr, true),
        ))
    }

    #[instrument]
    async fn activity_generate(
        &mut self,
        name: String,
        namespace: String,
        activity: Option<String>,
        domaintype: Option<String>,
    ) -> Result<ApiResponse, ApiError> {
        self.ensure_namespace(&namespace).await?;

        let mut connection = self.store.connection()?;
        let activity = self.store.get_activity_by_name_or_last_started(
            &mut connection,
            activity,
            Some(namespace.clone()),
        )?;

        let namespace = self.store.namespace_by_name(&mut connection, &namespace)?;
        let name = self
            .store
            .disambiguate_entity_name(&mut connection, &name)?;
        let id = ChronicleVocab::entity(&name);
        let create = ChronicleTransaction::GenerateEntity(GenerateEntity {
            namespace: namespace.clone(),
            id: id.clone().into(),
            activity: ChronicleVocab::activity(&activity.name).into(),
        });

        let mut to_apply = vec![create];

        if let Some(domaintype) = domaintype {
            let set_type = ChronicleTransaction::Domaintype(Domaintype::Entity {
                id: id.clone().into(),
                namespace,
                domaintype: Some(ChronicleVocab::domaintype(&domaintype).into()),
            });

            to_apply.push(set_type)
        }

        self.ledger_writer.submit(to_apply.iter().collect()).await?;

        Ok(ApiResponse::Prov(
            id,
            vec![self.store.apply(to_apply.iter().collect())?],
        ))
    }

    #[instrument]
    async fn activity_use(
        &mut self,
        name: String,
        namespace: String,
        activity: Option<String>,
        domaintype: Option<String>,
    ) -> Result<ApiResponse, ApiError> {
        let (id, to_apply) = {
            let mut connection = self.store.connection()?;

            self.ensure_namespace(&namespace).await?;
            let ns = self.store.namespace_by_name(&mut connection, &namespace)?;

            let activity = self.store.get_activity_by_name_or_last_started(
                &mut connection,
                activity,
                Some(namespace.clone()),
            )?;

            let name = self
                .store
                .disambiguate_entity_name(&mut connection, &name)?;
            let id = ChronicleVocab::entity(&name);

            let create = ChronicleTransaction::ActivityUses(ActivityUses {
                namespace: ns.clone(),
                id: id.clone().into(),
                activity: ChronicleVocab::activity(&activity.name).into(),
            });

            let mut to_apply = vec![create];

            if let Some(domaintype) = domaintype {
                let set_type = ChronicleTransaction::Domaintype(Domaintype::Entity {
                    id: id.clone().into(),
                    namespace: ns,
                    domaintype: Some(ChronicleVocab::domaintype(&domaintype).into()),
                });

                to_apply.push(set_type)
            }
            (id, to_apply)
        };

        self.ledger_writer.submit(to_apply.iter().collect()).await?;

        Ok(ApiResponse::Prov(
            id,
            vec![self.store.apply(to_apply.iter().collect())?],
        ))
    }

    pub fn as_ledger(self) -> W {
        self.ledger_writer
    }

    #[instrument]
    async fn create_activity(
        &mut self,
        name: String,
        namespace: String,
        domaintype: Option<String>,
    ) -> Result<ApiResponse, ApiError> {
        self.ensure_namespace(&namespace).await?;

        let mut connection = self.store.connection()?;
        let name = self
            .store
            .disambiguate_activity_name(&mut connection, &name)?;
        let namespace = self.store.namespace_by_name(&mut connection, &namespace)?;
        let id = ChronicleVocab::activity(&name);
        let create = ChronicleTransaction::CreateActivity(CreateActivity {
            namespace: namespace.clone(),
            id: id.clone().into(),
            name,
        });

        let mut to_apply = vec![create];

        if let Some(domaintype) = domaintype {
            let set_type = ChronicleTransaction::Domaintype(Domaintype::Activity {
                id: id.clone().into(),
                namespace,
                domaintype: Some(ChronicleVocab::domaintype(&domaintype).into()),
            });

            to_apply.push(set_type)
        }

        self.ledger_writer.submit(to_apply.iter().collect()).await?;

        Ok(ApiResponse::Prov(
            id,
            vec![self.store.apply(to_apply.iter().collect())?],
        ))
    }

    #[instrument]
    async fn create_agent(
        &mut self,
        name: String,
        namespace: String,
        domaintype: Option<String>,
    ) -> Result<ApiResponse, ApiError> {
        self.ensure_namespace(&namespace).await?;

        let mut connection = self.store.connection()?;
        let name = self.store.disambiguate_agent_name(&mut connection, &name)?;

        let iri = ChronicleVocab::agent(&name);

        let create = ChronicleTransaction::CreateAgent(CreateAgent {
            name: name.to_owned(),
            id: iri.clone().into(),
            namespace: self.store.namespace_by_name(&mut connection, &namespace)?,
        });

        let mut to_apply = vec![create];

        if let Some(domaintype) = domaintype {
            let set_type = ChronicleTransaction::Domaintype(Domaintype::Agent {
                id: ChronicleVocab::agent(&name).into(),
                namespace: self.store.namespace_by_name(&mut connection, &namespace)?,
                domaintype: Some(ChronicleVocab::domaintype(&domaintype).into()),
            });

            to_apply.push(set_type)
        }

        self.ledger_writer.submit(to_apply.iter().collect()).await?;

        Ok(ApiResponse::Prov(
            iri,
            vec![self.store.apply(to_apply.iter().collect())?],
        ))
    }

    #[instrument]
    async fn create_namespace(&mut self, name: &str) -> Result<ApiResponse, ApiError> {
        let uuid = (self.uuidsource)();
        let iri = ChronicleVocab::namespace(name, &uuid);

        let tx = ChronicleTransaction::CreateNamespace(CreateNamespace {
            id: iri.clone().into(),
            name: name.to_owned(),
            uuid,
        });

        self.ledger_writer.submit(vec![&tx]).await?;

        Ok(ApiResponse::Prov(iri, vec![self.store.apply(vec![&tx])?]))
    }

    #[instrument]
    async fn dispatch(&mut self, command: ApiCommand) -> Result<ApiResponse, ApiError> {
        match command {
            ApiCommand::NameSpace(NamespaceCommand::Create { name }) => {
                self.create_namespace(&name).await
            }
            ApiCommand::Agent(AgentCommand::Create {
                name,
                namespace,
                domaintype,
            }) => self.create_agent(name, namespace, domaintype).await,
            ApiCommand::Agent(AgentCommand::RegisterKey {
                name,
                namespace,
                registration,
            }) => self.register_key(name, namespace, registration).await,
            ApiCommand::Agent(AgentCommand::Use { name, namespace }) => {
                self.use_agent(name, namespace).await
            }
            ApiCommand::Activity(ActivityCommand::Create {
                name,
                namespace,
                domaintype,
            }) => self.create_activity(name, namespace, domaintype).await,
            ApiCommand::Activity(ActivityCommand::Start {
                name,
                namespace,
                time,
                agent,
            }) => self.start_activity(name, namespace, time, agent).await,
            ApiCommand::Activity(ActivityCommand::End {
                name,
                namespace,
                time,
                agent,
            }) => self.end_activity(name, namespace, time, agent).await,
            ApiCommand::Activity(ActivityCommand::Use {
                name,
                namespace,
                activity,
                domaintype,
            }) => {
                self.activity_use(name, namespace, activity, domaintype)
                    .await
            }
            ApiCommand::Activity(ActivityCommand::Generate {
                name,
                namespace,
                activity,
                domaintype,
            }) => {
                self.activity_generate(name, namespace, activity, domaintype)
                    .await
            }
            ApiCommand::Entity(EntityCommand::Attach {
                name,
                namespace,
                file,
                locator,
                agent,
            }) => {
                self.entity_attach(name, namespace, file, locator, agent)
                    .await
            }
            ApiCommand::Query(query) => self.query(query).await,
            ApiCommand::Sync(prov) => self.sync(prov).await,
        }
    }

    #[instrument]
    async fn end_activity(
        &mut self,
        name: Option<String>,
        namespace: Option<String>,
        time: Option<DateTime<Utc>>,
        agent: Option<String>,
    ) -> Result<ApiResponse, ApiError> {
        let mut connection = self.store.connection()?;
        let activity =
            self.store
                .get_activity_by_name_or_last_started(&mut connection, name, namespace)?;
        let namespace = self
            .store
            .namespace_by_name(&mut connection, &activity.namespace)?;

        let agent = {
            if let Some(agent) = agent {
                self.store.agent_by_agent_name_and_namespace(
                    &mut connection,
                    &agent,
                    namespace.decompose().0,
                )?
            } else {
                self.store
                    .get_current_agent(&mut connection)
                    .map_err(|_| ApiError::NoCurrentAgent {})?
            }
        };

        let id = ChronicleVocab::activity(&activity.name);
        let tx = ChronicleTransaction::EndActivity(EndActivity {
            namespace,
            id: id.clone().into(),
            agent: ChronicleVocab::agent(&agent.name).into(),
            time: time.unwrap_or_else(Utc::now),
        });

        self.ledger_writer.submit(vec![&tx]).await?;

        Ok(ApiResponse::Prov(id, vec![self.store.apply(vec![&tx])?]))
    }

    /// Our resources all assume a namespace, or the default namspace, so automatically create it by name if it doesn't exist
    #[instrument]
    async fn ensure_namespace(&mut self, namespace: &str) -> Result<(), ApiError> {
        let mut connection = self.store.connection()?;
        let ns = self.store.namespace_by_name(&mut connection, namespace);

        if ns.is_err() {
            debug!(namespace, "Namespace does not exist, creating");
            self.create_namespace(namespace).await?;
        }

        Ok(())
    }

    #[instrument]
    async fn entity_attach(
        &mut self,
        name: String,
        namespace: String,
        file: PathOrFile,
        locator: Option<String>,
        agent: Option<String>,
    ) -> Result<ApiResponse, ApiError> {
        self.ensure_namespace(&namespace).await?;

        let mut connection = self.store.connection()?;
        let agent = agent
            .map(|agent| {
                self.store
                    .agent_by_agent_name_and_namespace(&mut connection, &agent, &namespace)
            })
            .unwrap_or_else(|| self.store.get_current_agent(&mut connection))?;

        let namespace = self.store.namespace_by_name(&mut connection, &namespace)?;
        let id = ChronicleVocab::entity(&name);
        let agentid = ChronicleVocab::agent(&agent.name).into();

        let signature: Signature = {
            let signer = self.keystore.agent_signing(&agentid)?;

            match file {
                PathOrFile::Path(ref path) => signer
                    .sign(&std::fs::read(path).map_err(|_| ApiError::CannotFindAttachment {})?),
                PathOrFile::File(mut file) => {
                    let mut buf = vec![];
                    Arc::get_mut(&mut file)
                        .unwrap()
                        .read_to_end(&mut buf)
                        .await
                        .map_err(|e| ApiError::FileUpload { source: e })?;

                    signer.sign(&*buf)
                }
            }
        };

        let tx = ChronicleTransaction::EntityAttach(EntityAttach {
            namespace,
            id: id.clone().into(),
            agent: agentid,
            signature: hex::encode_upper(signature),
            locator,
            signature_time: Utc::now(),
        });

        self.ledger_writer.submit(vec![&tx]).await?;

        Ok(ApiResponse::Prov(id, vec![self.store.apply(vec![&tx])?]))
    }

    async fn query(&self, query: QueryCommand) -> Result<ApiResponse, ApiError> {
        let mut connection = self.store.connection()?;
        let id = self
            .store
            .namespace_by_name(&mut connection, &query.namespace)?;
        Ok(ApiResponse::Prov(
            IriBuf::new(id.as_str()).unwrap(),
            vec![self.store.prov_model_from(&mut connection, query)?],
        ))
    }

    async fn sync(&self, prov: ProvModel) -> Result<ApiResponse, ApiError> {
        self.store.apply_prov(&prov)?;

        Ok(ApiResponse::Unit)
    }

    #[instrument]
    async fn register_key(
        &mut self,
        name: String,
        namespace: String,
        registration: KeyRegistration,
    ) -> Result<ApiResponse, ApiError> {
        let mut connection = self.store.connection()?;
        self.ensure_namespace(&namespace).await?;
        let namespaceid = self.store.namespace_by_name(&mut connection, &namespace)?;
        let id = ChronicleVocab::agent(&name);
        match registration {
            KeyRegistration::Generate => {
                self.keystore.generate_agent(&id.clone().into())?;
            }
            KeyRegistration::ImportSigning(KeyImport::FromPath { path }) => self
                .keystore
                .import_agent(&id.clone().into(), Some(&path), None)?,
            KeyRegistration::ImportSigning(KeyImport::FromPEMBuffer { buffer }) => self
                .keystore
                .store_agent(&id.clone().into(), Some(&buffer), None)?,
            KeyRegistration::ImportVerifying(KeyImport::FromPath { path }) => self
                .keystore
                .import_agent(&id.clone().into(), None, Some(&path))?,
            KeyRegistration::ImportVerifying(KeyImport::FromPEMBuffer { buffer }) => self
                .keystore
                .store_agent(&id.clone().into(), None, Some(&buffer))?,
        }

        let tx = ChronicleTransaction::RegisterKey(RegisterKey {
            id: id.clone().into(),
            name,
            namespace: namespaceid,
            publickey: hex::encode(
                self.keystore
                    .agent_verifying(&id.clone().into())?
                    .to_bytes(),
            ),
        });

        self.ledger_writer.submit(vec![&tx]).await?;

        Ok(ApiResponse::Prov(id, vec![self.store.apply(vec![&tx])?]))
    }

    #[instrument]
    async fn start_activity(
        &mut self,
        name: String,
        namespace: String,
        time: Option<DateTime<Utc>>,
        agent: Option<String>,
    ) -> Result<ApiResponse, ApiError> {
        let mut connection = self.store.connection()?;
        let agent = {
            if let Some(agent) = agent {
                self.store
                    .agent_by_agent_name_and_namespace(&mut connection, &agent, &namespace)?
            } else {
                self.store
                    .get_current_agent(&mut connection)
                    .map_err(|_| ApiError::NoCurrentAgent {})?
            }
        };

        let name = self
            .store
            .disambiguate_activity_name(&mut connection, &name)?;
        let namespace = self.store.namespace_by_name(&mut connection, &namespace)?;
        let id = ChronicleVocab::activity(&name);
        let tx = ChronicleTransaction::StartActivity(StartActivity {
            namespace,
            id: id.clone().into(),
            agent: ChronicleVocab::agent(&agent.name).into(),
            time: time.unwrap_or_else(Utc::now),
        });

        self.ledger_writer.submit(vec![&tx]).await?;

        Ok(ApiResponse::Prov(id, vec![self.store.apply(vec![&tx])?]))
    }

    #[instrument]
    async fn use_agent(&self, name: String, namespace: String) -> Result<ApiResponse, ApiError> {
        let mut connection = self.store.connection()?;
        self.store.use_agent(&mut connection, name, namespace)?;

        Ok(ApiResponse::Unit)
    }
}

#[cfg(test)]
mod test {
    use std::{net::SocketAddr, str::FromStr};

    use chrono::{TimeZone, Utc};
    use common::{commands::ApiResponse, ledger::InMemLedger, prov::ProvModel};
    use tempfile::TempDir;
    use tracing::Level;
    use uuid::Uuid;

    use crate::{Api, ApiDispatch, ApiError};

    use common::commands::{
        ActivityCommand, AgentCommand, ApiCommand, KeyRegistration, NamespaceCommand,
    };

    struct TestDispatch(ApiDispatch, Vec<ProvModel>);

    impl TestDispatch {
        pub async fn dispatch(&mut self, command: ApiCommand) -> Result<(), ApiError> {
            if let ApiResponse::Prov(_, mut prov) = self.0.dispatch(command).await? {
                self.1.append(&mut prov);
            }

            Ok(())
        }
    }

    fn test_api() -> TestDispatch {
        tracing_log::LogTracer::init_with_filter(tracing::log::LevelFilter::Trace).unwrap();
        tracing_subscriber::fmt()
            .pretty()
            .with_max_level(Level::TRACE)
            .try_init()
            .ok();

        let secretpath = TempDir::new().unwrap();

        let mut ledger = InMemLedger::new();
        let reader = ledger.reader();

        let (dispatch, _ui) = Api::new(
            SocketAddr::from_str("0.0.0.0:8080").unwrap(),
            "file::memory:?cache=shared",
            ledger,
            reader,
            &secretpath.into_path(),
            || Uuid::parse_str("5a0ab5b8-eeb7-4812-9fe3-6dd69bd20cea").unwrap(),
        )
        .unwrap();

        TestDispatch(dispatch, vec![])
    }

    #[tokio::test]
    async fn create_namespace() {
        let mut api = test_api();

        api.dispatch(ApiCommand::NameSpace(NamespaceCommand::Create {
            name: "testns".to_owned(),
        }))
        .await
        .unwrap();

        insta::assert_yaml_snapshot!(api.1);
    }

    #[tokio::test]
    async fn create_agent() {
        let mut api = test_api();

        api.dispatch(ApiCommand::NameSpace(NamespaceCommand::Create {
            name: "testns".to_owned(),
        }))
        .await
        .unwrap();

        api.dispatch(ApiCommand::Agent(AgentCommand::Create {
            name: "testagent".to_owned(),
            namespace: "testns".to_owned(),
            domaintype: Some("testtype".to_owned()),
        }))
        .await
        .unwrap();

        insta::assert_yaml_snapshot!(api.1);
    }

    #[tokio::test]
    async fn agent_public_key() {
        let mut api = test_api();

        api.dispatch(ApiCommand::NameSpace(NamespaceCommand::Create {
            name: "testns".to_owned(),
        }))
        .await
        .unwrap();

        api.dispatch(ApiCommand::Agent(AgentCommand::RegisterKey {
            name: "testagent".to_owned(),
            namespace: "testns".to_owned(),
            registration: KeyRegistration::Generate,
        }))
        .await
        .unwrap();

        insta::assert_yaml_snapshot!(api.1, {
            ".*.publickey" => "[public]"
        });
    }

    #[tokio::test]
    async fn create_activity() {
        let mut api = test_api();

        api.dispatch(ApiCommand::Activity(ActivityCommand::Create {
            name: "testactivity".to_owned(),
            namespace: "testns".to_owned(),
            domaintype: Some("testtype".to_owned()),
        }))
        .await
        .unwrap();

        insta::assert_yaml_snapshot!(api.1);
    }

    #[tokio::test]
    async fn start_activity() {
        let mut api = test_api();

        api.dispatch(ApiCommand::Agent(AgentCommand::Create {
            name: "testagent".to_owned(),
            namespace: "testns".to_owned(),
            domaintype: Some("testtype".to_owned()),
        }))
        .await
        .unwrap();

        api.dispatch(ApiCommand::Agent(AgentCommand::Use {
            name: "testagent".to_owned(),
            namespace: "testns".to_owned(),
        }))
        .await
        .unwrap();

        api.dispatch(ApiCommand::Activity(ActivityCommand::Start {
            name: "testactivity".to_owned(),
            namespace: "testns".to_owned(),
            time: Some(Utc.ymd(2014, 7, 8).and_hms(9, 10, 11)),
            agent: None,
        }))
        .await
        .unwrap();

        insta::assert_yaml_snapshot!(api.1);
    }

    #[tokio::test]
    async fn end_activity() {
        let mut api = test_api();

        api.dispatch(ApiCommand::Agent(AgentCommand::Create {
            name: "testagent".to_owned(),
            namespace: "testns".to_owned(),
            domaintype: Some("testtype".to_owned()),
        }))
        .await
        .unwrap();

        api.dispatch(ApiCommand::Agent(AgentCommand::Use {
            name: "testagent".to_owned(),
            namespace: "testns".to_owned(),
        }))
        .await
        .unwrap();

        api.dispatch(ApiCommand::Activity(ActivityCommand::Start {
            name: "testactivity".to_owned(),
            namespace: "testns".to_owned(),
            time: Some(Utc.ymd(2014, 7, 8).and_hms(9, 10, 11)),
            agent: None,
        }))
        .await
        .unwrap();

        api.dispatch(ApiCommand::Activity(ActivityCommand::End {
            name: None,
            namespace: None,
            time: Some(Utc.ymd(2014, 7, 8).and_hms(9, 10, 11)),
            agent: None,
        }))
        .await
        .unwrap();

        insta::assert_yaml_snapshot!(api.1);
    }

    #[tokio::test]
    async fn activity_use() {
        let mut api = test_api();

        api.dispatch(ApiCommand::Agent(AgentCommand::Create {
            name: "testagent".to_owned(),
            namespace: "testns".to_owned(),
            domaintype: Some("testtype".to_owned()),
        }))
        .await
        .unwrap();

        api.dispatch(ApiCommand::Agent(AgentCommand::Use {
            name: "testagent".to_owned(),
            namespace: "testns".to_owned(),
        }))
        .await
        .unwrap();

        api.dispatch(ApiCommand::Activity(ActivityCommand::Create {
            name: "testactivity".to_owned(),
            namespace: "testns".to_owned(),
            domaintype: Some("testtype".to_owned()),
        }))
        .await
        .unwrap();

        api.dispatch(ApiCommand::Activity(ActivityCommand::Use {
            name: "testactivity".to_owned(),
            namespace: "testns".to_owned(),
            domaintype: Some("testtype".to_owned()),
            activity: None,
        }))
        .await
        .unwrap();

        api.dispatch(ApiCommand::Activity(ActivityCommand::Use {
            name: "testactivity".to_owned(),
            namespace: "testns".to_owned(),
            domaintype: Some("testtype".to_owned()),
            activity: None,
        }))
        .await
        .unwrap();

        api.dispatch(ApiCommand::Activity(ActivityCommand::End {
            name: None,
            namespace: None,
            time: Some(Utc.ymd(2014, 7, 8).and_hms(9, 10, 11)),
            agent: None,
        }))
        .await
        .unwrap();

        // Note that use should be idempotent as the name will be unique
        insta::assert_yaml_snapshot!(api.1);
    }

    #[tokio::test]
    async fn activity_generate() {
        let mut api = test_api();

        api.dispatch(ApiCommand::Activity(ActivityCommand::Create {
            name: "testactivity".to_owned(),
            namespace: "testns".to_owned(),
            domaintype: Some("testtype".to_owned()),
        }))
        .await
        .unwrap();

        api.dispatch(ApiCommand::Activity(ActivityCommand::Generate {
            name: "testactivity".to_owned(),
            namespace: "testns".to_owned(),
            domaintype: Some("testtype".to_owned()),
            activity: None,
        }))
        .await
        .unwrap();

        api.dispatch(ApiCommand::Activity(ActivityCommand::Generate {
            name: "testactivity".to_owned(),
            namespace: "testns".to_owned(),
            domaintype: Some("testtype".to_owned()),
            activity: None,
        }))
        .await
        .unwrap();

        // Note that generate should be idempotent as the name will be unique
        insta::assert_yaml_snapshot!(api.1);
    }

    #[tokio::test]
    async fn many_activities() {
        let mut api = test_api();

        for _ in 0..100 {
            api.dispatch(ApiCommand::Activity(ActivityCommand::Create {
                name: "testactivity".to_owned(),
                namespace: "testns".to_owned(),
                domaintype: Some("testtype".to_owned()),
            }))
            .await
            .unwrap();
        }

        insta::assert_yaml_snapshot!(api.1);
    }
}
