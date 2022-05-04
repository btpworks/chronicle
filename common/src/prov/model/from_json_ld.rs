use chrono::{DateTime, Utc};
use futures::TryFutureExt;
use iref::{AsIri, IriBuf};
use json::JsonValue;
use json_ld::{util::AsJson, Document, Indexed, JsonContext, NoLoader, Node, Reference};

use crate::{
    attributes::{Attribute, Attributes},
    prov::{
        operations::DerivationType,
        vocab::{Chronicle, Prov},
        ActivityId, AgentId, AttachmentId, EntityId, IdentityId, NamespaceId,
    },
};

use super::{Activity, Agent, Attachment, Entity, Identity, ProcessorError, ProvModel};

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
    Ok(NamespaceId::new(
        extract_scalar_prop(&Chronicle::HasNamespace, agent)?
            .id()
            .ok_or(ProcessorError::MissingId {
                object: agent.as_json(),
            })?
            .to_string(),
    ))
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
            .filter(|x| x.as_str().contains("domaintype"))
            .map(|x| x.into())
            .next();

        Ok(Attributes {
            typ,
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

        self.namespace_context(&NamespaceId::new(ns.as_str()));

        Ok(())
    }

    fn apply_node_as_agent(&mut self, agent: &Node) -> Result<(), ProcessorError> {
        let id = AgentId::new(
            agent
                .id()
                .ok_or_else(|| ProcessorError::MissingId {
                    object: agent.as_json(),
                })?
                .to_string(),
        );

        let namespaceid = extract_namespace(agent)?;
        self.namespace_context(&namespaceid);

        let attributes = Self::extract_attributes(agent)?;

        for delegated in extract_reference_ids(&Prov::ActedOnBehalfOf, agent)?
            .into_iter()
            .map(|id| AgentId::new(id.as_str()))
        {
            self.acted_on_behalf_of(namespaceid.clone(), id.clone(), delegated, None);
        }

        for identity in extract_reference_ids(&Chronicle::HasIdentity, agent)?
            .into_iter()
            .map(|id| IdentityId::new(id.as_str()))
        {
            self.has_identity(namespaceid.clone(), &id, &identity);
        }

        for identity in extract_reference_ids(&Chronicle::HadIdentity, agent)?
            .into_iter()
            .map(|id| IdentityId::new(id.as_str()))
        {
            self.had_identity(namespaceid.clone(), &id, &identity);
        }

        let agent = Agent::exists(namespaceid, id).has_attributes(attributes);

        self.add_agent(agent);

        Ok(())
    }

    fn apply_node_as_activity(&mut self, activity: &Node) -> Result<(), ProcessorError> {
        let id = ActivityId::new(
            activity
                .id()
                .ok_or_else(|| ProcessorError::MissingId {
                    object: activity.as_json(),
                })?
                .to_string(),
        );

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
            .map(|id| EntityId::new(id.as_str()));

        let wasassociatedwith = extract_reference_ids(&Prov::WasAssociatedWith, activity)?
            .into_iter()
            .map(|id| AgentId::new(id.as_str()));

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

        let id = IdentityId::new(
            identity
                .id()
                .ok_or_else(|| ProcessorError::MissingId {
                    object: identity.as_json(),
                })?
                .to_string(),
        );

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
        let id = AttachmentId::new(
            attachment
                .id()
                .ok_or_else(|| ProcessorError::MissingId {
                    object: attachment.as_json(),
                })?
                .to_string(),
        );

        let signer = extract_reference_ids(&Chronicle::SignedBy, attachment)?
            .into_iter()
            .map(|id| IdentityId::new(id.as_str()))
            .next()
            .ok_or_else(|| ProcessorError::MissingId {
                object: attachment.as_json(),
            })?;

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
        let id = EntityId::new(
            entity
                .id()
                .ok_or_else(|| ProcessorError::MissingId {
                    object: entity.as_json(),
                })?
                .to_string(),
        );

        let namespaceid = extract_namespace(entity)?;
        self.namespace_context(&namespaceid);

        let generatedby = extract_reference_ids(&Prov::WasGeneratedBy, entity)?
            .into_iter()
            .map(|id| ActivityId::new(id.as_str()));

        for attachment in extract_reference_ids(&Chronicle::HasAttachment, entity)?
            .into_iter()
            .map(|id| AttachmentId::new(id.as_str()))
        {
            self.has_attachment(namespaceid.clone(), id.clone(), &attachment);
        }

        for attachment in extract_reference_ids(&Chronicle::HadAttachment, entity)?
            .into_iter()
            .map(|id| AttachmentId::new(id.as_str()))
        {
            self.had_attachment(namespaceid.clone(), id.clone(), &attachment);
        }

        for derived in extract_reference_ids(&Prov::WasDerivedFrom, entity)?
            .into_iter()
            .map(|id| EntityId::new(id.as_str()))
        {
            self.was_derived_from(namespaceid.clone(), None, derived, id.clone(), None);
        }

        for derived in extract_reference_ids(&Prov::WasQuotedFrom, entity)?
            .into_iter()
            .map(|id| EntityId::new(id.as_str()))
        {
            self.was_derived_from(
                namespaceid.clone(),
                Some(DerivationType::quotation()),
                derived,
                id.clone(),
                None,
            );
        }

        for derived in extract_reference_ids(&Prov::WasRevisionOf, entity)?
            .into_iter()
            .map(|id| EntityId::new(id.as_str()))
        {
            self.was_derived_from(
                namespaceid.clone(),
                Some(DerivationType::revision()),
                derived,
                id.clone(),
                None,
            );
        }

        for derived in extract_reference_ids(&Prov::HadPrimarySource, entity)?
            .into_iter()
            .map(|id| EntityId::new(id.as_str()))
        {
            self.was_derived_from(
                namespaceid.clone(),
                Some(DerivationType::primary_source()),
                derived,
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
