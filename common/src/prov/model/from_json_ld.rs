use chrono::{DateTime, Utc};
use futures::TryFutureExt;
use iref::{AsIri, Iri, IriBuf};
use json::JsonValue;
use json_ld::{util::AsJson, Document, Indexed, JsonContext, NoLoader, Node, Reference};

use crate::{
    attributes::{Attribute, Attributes},
    prov::{
        operations::{
            ActsOnBehalfOf, ChronicleOperation, CreateAgent, CreateNamespace, DerivationType,
        },
        vocab::{Chronicle, ChronicleOperations, Prov},
        ActivityId, AgentId, AttachmentId, DomaintypeId, EntityId, IdentityId, NamePart,
        NamespaceId, UuidPart,
    },
};

use super::{
    Activity, Agent, Attachment, Entity, ExpandedJson, Identity, ProcessorError, ProvModel,
};

fn extract_reference_ids(iri: &dyn AsIri, node: &Node) -> Result<Vec<IriBuf>, ProcessorError> {
    let ids: Result<Vec<_>, _> = node
        .get(&Reference::Id(iri.as_iri().into()))
        .map(|o| {
            o.id().ok_or_else(|| ProcessorError::MissingId {
                object: node.as_json(),
            })
        })
        .map(|id| {
            id.and_then(|id| {
                id.as_iri().ok_or_else(|| ProcessorError::MissingId {
                    object: node.as_json(),
                })
            })
        })
        .map(|id| id.map(|id| id.to_owned()))
        .collect();

    ids
}

fn extract_scalar_prop<'a>(
    iri: &dyn AsIri,
    node: &'a Node,
) -> Result<&'a Indexed<json_ld::object::Object>, ProcessorError> {
    node.get_any(&Reference::Id(iri.as_iri().into()))
        .ok_or_else(|| ProcessorError::MissingProperty {
            iri: iri.as_iri().as_str().to_string(),
            object: node.as_json(),
        })
}

fn extract_namespace(agent: &Node) -> Result<NamespaceId, ProcessorError> {
    Ok(NamespaceId::try_from(Iri::from_str(
        extract_scalar_prop(&Chronicle::HasNamespace, agent)?
            .id()
            .ok_or(ProcessorError::MissingId {
                object: agent.as_json(),
            })?
            .as_str(),
    )?)?)
}

impl ProvModel {
    pub async fn apply_json_ld_bytes(self, buf: &[u8]) -> Result<Self, ProcessorError> {
        self.apply_json_ld(json::parse(std::str::from_utf8(buf)?)?)
            .await
    }

    /// Take a Json-Ld input document, assuming it is in compact form, expand it and apply the state to the prov model
    /// Replace @context with our resource context
    /// We rely on reified @types, so subclassing must also include supertypes
    pub async fn apply_json_ld(mut self, mut json: JsonValue) -> Result<Self, ProcessorError> {
        json.remove("@context");
        json.insert("@context", crate::context::PROV.clone()).ok();

        let output = json
            .expand::<JsonContext, _>(&mut NoLoader)
            .map_err(|e| ProcessorError::Expansion {
                inner: e.to_string(),
            })
            .await?;

        for o in output {
            let o = o
                .try_cast::<Node>()
                .map_err(|_| ProcessorError::NotANode {})?
                .into_inner();
            if o.has_type(&Reference::Id(Chronicle::Namespace.as_iri().into())) {
                self.apply_node_as_namespace(&o)?;
            }
            if o.has_type(&Reference::Id(Prov::Agent.as_iri().into())) {
                self.apply_node_as_agent(&o)?;
            } else if o.has_type(&Reference::Id(Prov::Activity.as_iri().into())) {
                self.apply_node_as_activity(&o)?;
            } else if o.has_type(&Reference::Id(Prov::Entity.as_iri().into())) {
                self.apply_node_as_entity(&o)?;
            } else if o.has_type(&Reference::Id(Chronicle::Identity.as_iri().into())) {
                self.apply_node_as_identity(&o)?;
            } else if o.has_type(&Reference::Id(Chronicle::HasAttachment.as_iri().into())) {
                self.apply_node_as_attachment(&o)?;
            }
        }

        Ok(self)
    }

    /// Extract the types and find the first that is not prov::, as we currently only alow zero or one domain types
    /// this should be sufficient
    fn extract_attributes(node: &Node) -> Result<Attributes, ProcessorError> {
        let typ = node
            .types()
            .iter()
            .filter_map(|x| x.as_iri())
            .find(|x| x.as_str().contains("domaintype"))
            .map(|iri| Ok::<_, ProcessorError>(DomaintypeId::try_from(iri.as_iri())?))
            .transpose();

        Ok(Attributes {
            typ: typ?,
            attributes: node
                .get(&Reference::Id(Chronicle::Value.as_iri().into()))
                .map(|o| {
                    let serde_object = serde_json::from_str(&*o.as_json()["@value"].to_string())?;

                    if let serde_json::Value::Object(object) = serde_object {
                        Ok(object
                            .into_iter()
                            .map(|(typ, value)| Attribute { typ, value })
                            .collect::<Vec<_>>())
                    } else {
                        Err(ProcessorError::NotAnObject {})
                    }
                })
                .collect::<Result<Vec<_>, _>>()?
                .into_iter()
                .flatten()
                .map(|attr| (attr.typ.clone(), attr))
                .collect(),
        })
    }

    fn apply_node_as_namespace(&mut self, ns: &Node) -> Result<(), ProcessorError> {
        let ns = ns.id().ok_or_else(|| ProcessorError::MissingId {
            object: ns.as_json(),
        })?;

        self.namespace_context(&NamespaceId::try_from(Iri::from_str(ns.as_str())?)?);

        Ok(())
    }

    fn apply_node_as_agent(&mut self, agent: &Node) -> Result<(), ProcessorError> {
        let id = AgentId::try_from(Iri::from_str(
            agent
                .id()
                .ok_or_else(|| ProcessorError::MissingId {
                    object: agent.as_json(),
                })?
                .as_str(),
        )?)?;

        let namespaceid = extract_namespace(agent)?;
        self.namespace_context(&namespaceid);

        let attributes = Self::extract_attributes(agent)?;

        for delegated in extract_reference_ids(&Prov::ActedOnBehalfOf, agent)?
            .into_iter()
            .map(|id| AgentId::try_from(id.as_iri()))
        {
            self.acted_on_behalf_of(namespaceid.clone(), id.clone(), delegated?, None);
        }

        for identity in extract_reference_ids(&Chronicle::HasIdentity, agent)?
            .into_iter()
            .map(|id| IdentityId::try_from(id.as_iri()))
        {
            self.has_identity(namespaceid.clone(), &id, &identity?);
        }

        for identity in extract_reference_ids(&Chronicle::HadIdentity, agent)?
            .into_iter()
            .map(|id| IdentityId::try_from(id.as_iri()))
        {
            self.had_identity(namespaceid.clone(), &id, &identity?);
        }

        let agent = Agent::exists(namespaceid, id).has_attributes(attributes);

        self.add_agent(agent);

        Ok(())
    }

    fn apply_node_as_activity(&mut self, activity: &Node) -> Result<(), ProcessorError> {
        let id = ActivityId::try_from(Iri::from_str(
            activity
                .id()
                .ok_or_else(|| ProcessorError::MissingId {
                    object: activity.as_json(),
                })?
                .as_str(),
        )?)?;

        let namespaceid = extract_namespace(activity)?;
        self.namespace_context(&namespaceid);

        let started = extract_scalar_prop(&Prov::StartedAtTime, activity)
            .ok()
            .and_then(|x| x.as_str().map(DateTime::parse_from_rfc3339));

        let ended = extract_scalar_prop(&Prov::EndedAtTime, activity)
            .ok()
            .and_then(|x| x.as_str().map(DateTime::parse_from_rfc3339));

        let used = extract_reference_ids(&Prov::Used, activity)?
            .into_iter()
            .map(|id| EntityId::try_from(id.as_iri()))
            .collect::<Result<Vec<_>, _>>()?;

        let wasassociatedwith = extract_reference_ids(&Prov::WasAssociatedWith, activity)?
            .into_iter()
            .map(|id| AgentId::try_from(id.as_iri()))
            .collect::<Result<Vec<_>, _>>()?;

        let attributes = Self::extract_attributes(activity)?;

        let mut activity = Activity::exists(namespaceid.clone(), id).has_attributes(attributes);

        if let Some(started) = started {
            activity.started = Some(DateTime::<Utc>::from(started?));
        }

        if let Some(ended) = ended {
            activity.ended = Some(DateTime::<Utc>::from(ended?));
        }

        for entity in used {
            self.used(namespaceid.clone(), &activity.id, &entity);
        }

        for agent in wasassociatedwith {
            self.was_associated_with(&namespaceid, &activity.id, &agent);
        }

        self.add_activity(activity);

        Ok(())
    }

    fn apply_node_as_identity(&mut self, identity: &Node) -> Result<(), ProcessorError> {
        let namespaceid = extract_namespace(identity)?;

        let id = IdentityId::try_from(Iri::from_str(
            identity
                .id()
                .ok_or_else(|| ProcessorError::MissingId {
                    object: identity.as_json(),
                })?
                .as_str(),
        )?)?;

        let public_key = extract_scalar_prop(&Chronicle::PublicKey, identity)
            .ok()
            .and_then(|x| x.as_str().map(|x| x.to_string()))
            .ok_or_else(|| ProcessorError::MissingProperty {
                iri: Chronicle::PublicKey.as_iri().to_string(),
                object: identity.as_json(),
            })?;

        self.add_identity(Identity {
            id,
            namespaceid,
            public_key,
        });

        Ok(())
    }

    fn apply_node_as_attachment(&mut self, attachment: &Node) -> Result<(), ProcessorError> {
        let namespaceid = extract_namespace(attachment)?;

        let id = AttachmentId::try_from(Iri::from_str(
            attachment
                .id()
                .ok_or_else(|| ProcessorError::MissingId {
                    object: attachment.as_json(),
                })?
                .as_str(),
        )?)?;

        let signer = extract_reference_ids(&Chronicle::SignedBy, attachment)?
            .into_iter()
            .next()
            .ok_or_else(|| ProcessorError::MissingId {
                object: attachment.as_json(),
            })
            .map(|id| IdentityId::try_from(id.as_iri()))??;

        let signature = extract_scalar_prop(&Chronicle::Signature, attachment)
            .ok()
            .and_then(|x| x.as_str())
            .ok_or_else(|| ProcessorError::MissingProperty {
                iri: Chronicle::Signature.as_iri().to_string(),
                object: attachment.as_json(),
            })?
            .to_owned();

        let signature_time = extract_scalar_prop(&Chronicle::SignedAtTime, attachment)
            .ok()
            .and_then(|x| x.as_str().map(DateTime::parse_from_rfc3339))
            .ok_or_else(|| ProcessorError::MissingProperty {
                iri: Chronicle::SignedAtTime.as_iri().to_string(),
                object: attachment.as_json(),
            })??;

        let locator = extract_scalar_prop(&Chronicle::Locator, attachment)
            .ok()
            .and_then(|x| x.as_str());

        self.add_attachment(Attachment {
            namespaceid,
            id,
            signature,
            signer,
            locator: locator.map(|x| x.to_owned()),
            signature_time: signature_time.into(),
        });

        Ok(())
    }

    fn apply_node_as_entity(&mut self, entity: &Node) -> Result<(), ProcessorError> {
        let id = EntityId::try_from(Iri::from_str(
            entity
                .id()
                .ok_or_else(|| ProcessorError::MissingId {
                    object: entity.as_json(),
                })?
                .as_str(),
        )?)?;

        let namespaceid = extract_namespace(entity)?;
        self.namespace_context(&namespaceid);

        let generatedby = extract_reference_ids(&Prov::WasGeneratedBy, entity)?
            .into_iter()
            .map(|id| ActivityId::try_from(id.as_iri()))
            .collect::<Result<Vec<_>, _>>()?;

        for attachment in extract_reference_ids(&Chronicle::HasAttachment, entity)?
            .into_iter()
            .map(|id| AttachmentId::try_from(id.as_iri()))
        {
            self.has_attachment(namespaceid.clone(), id.clone(), &attachment?);
        }

        for attachment in extract_reference_ids(&Chronicle::HadAttachment, entity)?
            .into_iter()
            .map(|id| AttachmentId::try_from(id.as_iri()))
        {
            self.had_attachment(namespaceid.clone(), id.clone(), &attachment?);
        }

        for derived in extract_reference_ids(&Prov::WasDerivedFrom, entity)?
            .into_iter()
            .map(|id| EntityId::try_from(id.as_iri()))
        {
            self.was_derived_from(namespaceid.clone(), None, derived?, id.clone(), None);
        }

        for derived in extract_reference_ids(&Prov::WasQuotedFrom, entity)?
            .into_iter()
            .map(|id| EntityId::try_from(id.as_iri()))
        {
            self.was_derived_from(
                namespaceid.clone(),
                Some(DerivationType::quotation()),
                derived?,
                id.clone(),
                None,
            );
        }

        for derived in extract_reference_ids(&Prov::WasRevisionOf, entity)?
            .into_iter()
            .map(|id| EntityId::try_from(id.as_iri()))
        {
            self.was_derived_from(
                namespaceid.clone(),
                Some(DerivationType::revision()),
                derived?,
                id.clone(),
                None,
            );
        }

        for derived in extract_reference_ids(&Prov::HadPrimarySource, entity)?
            .into_iter()
            .map(|id| EntityId::try_from(id.as_iri()))
        {
            self.was_derived_from(
                namespaceid.clone(),
                Some(DerivationType::primary_source()),
                derived?,
                id.clone(),
                None,
            );
        }

        for activity in generatedby {
            self.was_generated_by(namespaceid.clone(), &id, &activity);
        }

        let attributes = Self::extract_attributes(entity)?;
        self.add_entity(Entity::exists(namespaceid, id).has_attributes(attributes));

        Ok(())
    }
}

fn operation_namespace(o: &Node) -> NamespaceId {
    let mut uuid_objects = o.get(&Reference::Id(
        ChronicleOperations::NamespaceUuid.as_iri().into(),
    ));
    let uuid = uuid_objects.next().unwrap().as_str().unwrap();
    let mut name_objects = o.get(&Reference::Id(
        ChronicleOperations::NamespaceName.as_iri().into(),
    ));
    let name = name_objects.next().unwrap().as_str().unwrap();
    let uuid = uuid::Uuid::parse_str(uuid).unwrap();
    NamespaceId::from_name(name, uuid)
}

fn operation_agent(o: &Node) -> AgentId {
    let mut name_objects = o.get(&Reference::Id(
        ChronicleOperations::AgentName.as_iri().into(),
    ));
    let name = name_objects.next().unwrap().as_str().unwrap();
    AgentId::from_name(name)
}

fn operation_delegate(o: &Node) -> AgentId {
    let mut name_objects = o.get(&Reference::Id(
        ChronicleOperations::DelegateId.as_iri().into(),
    ));
    let name = name_objects.next().unwrap().as_str().unwrap();
    AgentId::from_name(name)
}

fn operation_activity_id(o: &Node) -> Option<ActivityId> {
    let mut name_objects = o.get(&Reference::Id(
        ChronicleOperations::ActivityName.as_iri().into(),
    ));
    match name_objects.next() {
        Some(object) => Some(ActivityId::from_name(object.as_str().unwrap())),
        None => return None,
    }
}

impl ChronicleOperation {
    pub async fn from_json(ExpandedJson(json): ExpandedJson) -> Result<Self, ProcessorError> {
        let output = json
            .expand::<JsonContext, _>(&mut NoLoader)
            .map_err(|e| ProcessorError::Expansion {
                inner: e.to_string(),
            })
            .await?;
        assert!(output.len() == 1);
        if let Some(object) = output.into_iter().next() {
            let o = object
                .try_cast::<Node>()
                .map_err(|_| ProcessorError::NotANode {})?
                .into_inner();
            let id = o.id().unwrap().as_str();
            assert!(id == "_:n1");
            if o.has_type(&Reference::Id(
                ChronicleOperations::CreateNamespace.as_iri().into(),
            )) {
                let namespace = operation_namespace(&o);
                let name = namespace.name_part().to_owned();
                let uuid = namespace.uuid_part().to_owned();
                Ok(ChronicleOperation::CreateNamespace(CreateNamespace {
                    id: namespace,
                    name,
                    uuid,
                }))
            } else if o.has_type(&Reference::Id(
                ChronicleOperations::CreateAgent.as_iri().into(),
            )) {
                let namespace = operation_namespace(&o);
                let agent = operation_agent(&o);
                // let mut agent_name_objects = o.get(&Reference::Id(
                //     ChronicleOperations::AgentName.as_iri().into(),
                // ));
                let name = agent.name_part();
                // let name = agent_name_objects.next().unwrap().as_str().unwrap();
                Ok(ChronicleOperation::CreateAgent(CreateAgent {
                    namespace,
                    name: name.into(),
                }))
            } else if o.has_type(&Reference::Id(
                ChronicleOperations::AgentActsOnBehalfOf.as_iri().into(),
            )) {
                let namespace = operation_namespace(&o);
                let id = operation_agent(&o);
                let delegate_id = operation_delegate(&o);
                let activity_id = operation_activity_id(&o);
                Ok(ChronicleOperation::AgentActsOnBehalfOf(ActsOnBehalfOf {
                    namespace,
                    id,
                    delegate_id,
                    activity_id,
                }))
            } else {
                unreachable!()
            }
        } else {
            Err(ProcessorError::NotANode {})
        }
    }
}

#[cfg(test)]
mod test {
    use uuid::Uuid;

    use crate::prov::{
        operations::{ActsOnBehalfOf, ChronicleOperation, CreateNamespace},
        to_json_ld::ToJson,
        ActivityId, AgentId, NamespaceId, ProcessorError,
    };

    #[tokio::test]
    async fn test_create_agent_acts_on_behalf_of_no_activity() -> Result<(), ProcessorError> {
        let uuid =
            Uuid::parse_str("a1a2a3a4-b1b2-c1c2-d1d2-d3d4d5d6d7d8").map_err(|e| eprintln!("{}", e));
        let namespace: NamespaceId = NamespaceId::from_name("testns", uuid.unwrap());
        let id = AgentId::from_name("test_agent");
        let delegate_id = AgentId::from_name("test_delegate");
        let activity_id = None;

        let operation: ChronicleOperation =
            ChronicleOperation::AgentActsOnBehalfOf(ActsOnBehalfOf {
                namespace,
                id,
                delegate_id,
                activity_id,
            });

        let serialized_operation = operation.to_json();
        let deserialized_operation = ChronicleOperation::from_json(serialized_operation).await?;
        assert!(
            ChronicleOperation::from_json(deserialized_operation.to_json()).await?
                == deserialized_operation
        );
        let operation_json = deserialized_operation.to_json();
        let x: serde_json::Value = serde_json::from_str(&operation_json.0.to_string())?;
        insta::assert_json_snapshot!(&x, @r###"
        [
          {
            "@id": "_:n1",
            "@type": "http://blockchaintp.com/chronicleoperations/ns#AgentActsOnBehalfOf",
            "http://blockchaintp.com/chronicleoperations/ns#AgentName": [
              {
                "@value": "test_agent"
              }
            ],
            "http://blockchaintp.com/chronicleoperations/ns#DelegateId": [
              {
                "@value": "test_delegate"
              }
            ],
            "http://blockchaintp.com/chronicleoperations/ns#NamespaceName": [
              {
                "@value": "testns"
              }
            ],
            "http://blockchaintp.com/chronicleoperations/ns#NamespaceUuid": [
              {
                "@value": "a1a2a3a4-b1b2-c1c2-d1d2-d3d4d5d6d7d8"
              }
            ]
          }
        ]
        "###);
        Ok(())
    }

    #[tokio::test]
    async fn test_create_agent_acts_on_behalf_of() -> Result<(), ProcessorError> {
        let uuid =
            Uuid::parse_str("a1a2a3a4-b1b2-c1c2-d1d2-d3d4d5d6d7d8").map_err(|e| eprintln!("{}", e));
        let namespace: NamespaceId = NamespaceId::from_name("testns", uuid.unwrap());
        let id = AgentId::from_name("test_agent");
        let delegate_id = AgentId::from_name("test_delegate");
        let activity_id = Some(ActivityId::from_name("test_activity"));

        let operation: ChronicleOperation =
            ChronicleOperation::AgentActsOnBehalfOf(ActsOnBehalfOf {
                namespace,
                id,
                delegate_id,
                activity_id,
            });

        let serialized_operation = operation.to_json();
        let deserialized_operation = ChronicleOperation::from_json(serialized_operation).await?;
        assert!(
            ChronicleOperation::from_json(deserialized_operation.to_json()).await?
                == deserialized_operation
        );
        let operation_json = deserialized_operation.to_json();
        let x: serde_json::Value = serde_json::from_str(&operation_json.0.to_string())?;
        insta::assert_json_snapshot!(&x, @r###"
        [
          {
            "@id": "_:n1",
            "@type": "http://blockchaintp.com/chronicleoperations/ns#AgentActsOnBehalfOf",
            "http://blockchaintp.com/chronicleoperations/ns#ActivityName": [
              {
                "@value": "test_activity"
              }
            ],
            "http://blockchaintp.com/chronicleoperations/ns#AgentName": [
              {
                "@value": "test_agent"
              }
            ],
            "http://blockchaintp.com/chronicleoperations/ns#DelegateId": [
              {
                "@value": "test_delegate"
              }
            ],
            "http://blockchaintp.com/chronicleoperations/ns#NamespaceName": [
              {
                "@value": "testns"
              }
            ],
            "http://blockchaintp.com/chronicleoperations/ns#NamespaceUuid": [
              {
                "@value": "a1a2a3a4-b1b2-c1c2-d1d2-d3d4d5d6d7d8"
              }
            ]
          }
        ]
        "###);
        Ok(())
    }

    #[tokio::test]
    async fn test_create_agent_from_json() -> Result<(), ProcessorError> {
        let uuid =
            Uuid::parse_str("a1a2a3a4-b1b2-c1c2-d1d2-d3d4d5d6d7d8").map_err(|e| eprintln!("{}", e));
        let namespace: NamespaceId = NamespaceId::from_name("testns", uuid.unwrap());
        let name: crate::prov::Name =
            crate::prov::NamePart::name_part(&crate::prov::AgentId::from_name("test_agent"))
                .clone();
        let operation: ChronicleOperation =
            super::ChronicleOperation::CreateAgent(crate::prov::operations::CreateAgent {
                namespace,
                name,
            });
        let serialized_operation = operation.to_json();
        let deserialized_operation = ChronicleOperation::from_json(serialized_operation).await?;
        assert!(
            ChronicleOperation::from_json(deserialized_operation.to_json()).await?
                == deserialized_operation
        );
        let operation_json = deserialized_operation.to_json();
        let x: serde_json::Value = serde_json::from_str(&operation_json.0.to_string())?;
        insta::assert_json_snapshot!(&x, @r###"
        [
          {
            "@id": "_:n1",
            "@type": "http://blockchaintp.com/chronicleoperations/ns#CreateAgent",
            "http://blockchaintp.com/chronicleoperations/ns#AgentName": [
              {
                "@value": "test_agent"
              }
            ],
            "http://blockchaintp.com/chronicleoperations/ns#NamespaceName": [
              {
                "@value": "testns"
              }
            ],
            "http://blockchaintp.com/chronicleoperations/ns#NamespaceUuid": [
              {
                "@value": "a1a2a3a4-b1b2-c1c2-d1d2-d3d4d5d6d7d8"
              }
            ]
          }
        ]
        "###);
        Ok(())
    }

    #[tokio::test]
    async fn test_create_namespace_from_json() -> Result<(), ProcessorError> {
        let name = "testns";
        let uuid =
            Uuid::parse_str("a1a2a3a4-b1b2-c1c2-d1d2-d3d4d5d6d7d8").map_err(|e| eprintln!("{}", e));
        let id = NamespaceId::from_name(name, uuid.unwrap());

        let operation =
            ChronicleOperation::CreateNamespace(CreateNamespace::new(id, name, uuid.unwrap()));
        let serialized_operation = operation.to_json();
        let deserialized_operation = ChronicleOperation::from_json(serialized_operation).await?;
        assert!(
            ChronicleOperation::from_json(deserialized_operation.to_json()).await?
                == deserialized_operation
        );
        let operation_json = deserialized_operation.to_json();
        let x: serde_json::Value = serde_json::from_str(&operation_json.0.to_string())?;
        insta::assert_json_snapshot!(&x, @r###"
        [
          {
            "@id": "_:n1",
            "@type": "http://blockchaintp.com/chronicleoperations/ns#CreateNamespace",
            "http://blockchaintp.com/chronicleoperations/ns#NamespaceName": [
              {
                "@value": "testns"
              }
            ],
            "http://blockchaintp.com/chronicleoperations/ns#NamespaceUuid": [
              {
                "@value": "a1a2a3a4-b1b2-c1c2-d1d2-d3d4d5d6d7d8"
              }
            ]
          }
        ]
        "###);
        Ok(())
    }
}
