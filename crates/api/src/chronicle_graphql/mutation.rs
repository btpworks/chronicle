//! Primitive mutation operations that are not in terms of particular domain types

use std::sync::Arc;

use async_graphql::{Context, Upload};
use chrono::{DateTime, Utc};
use common::{
    attributes::Attributes,
    commands::{
        ActivityCommand, AgentCommand, ApiCommand, ApiResponse, EntityCommand, KeyRegistration,
        PathOrFile,
    },
    prov::{operations::DerivationType, ActivityId, AgentId, EntityId, Role},
};

use crate::ApiDispatch;

use super::Submission;
pub async fn transaction_context<'a>(
    res: ApiResponse,
    _ctx: &Context<'a>,
) -> async_graphql::Result<Submission> {
    match res {
        ApiResponse::Submission { subject, tx_id, .. } => Ok(Submission {
            context: subject.to_string(),
            tx_id: tx_id.to_string(),
        }),
        _ => unreachable!(),
    }
}

async fn derivation<'a>(
    ctx: &Context<'a>,
    namespace: Option<String>,
    generated_entity: EntityId,
    used_entity: EntityId,
    derivation: Option<DerivationType>,
) -> async_graphql::Result<Submission> {
    let api = ctx.data_unchecked::<ApiDispatch>();

    let namespace = namespace.unwrap_or_else(|| "default".into()).into();

    let res = api
        .dispatch(ApiCommand::Entity(EntityCommand::Derive {
            id: generated_entity,
            namespace,
            activity: None,
            used_entity,
            derivation,
        }))
        .await?;

    transaction_context(res, ctx).await
}

pub async fn agent<'a>(
    ctx: &Context<'a>,
    external_id: String,
    namespace: Option<String>,
    attributes: Attributes,
) -> async_graphql::Result<Submission> {
    let api = ctx.data_unchecked::<ApiDispatch>();

    let namespace = namespace.unwrap_or_else(|| "default".to_owned());

    let res = api
        .dispatch(ApiCommand::Agent(AgentCommand::Create {
            external_id: external_id.into(),
            namespace: namespace.into(),
            attributes,
        }))
        .await?;

    transaction_context(res, ctx).await
}

pub async fn activity<'a>(
    ctx: &Context<'a>,
    external_id: String,
    namespace: Option<String>,
    attributes: Attributes,
) -> async_graphql::Result<Submission> {
    let api = ctx.data_unchecked::<ApiDispatch>();

    let namespace = namespace.unwrap_or_else(|| "default".to_owned());

    let res = api
        .dispatch(ApiCommand::Activity(ActivityCommand::Create {
            external_id: external_id.into(),
            namespace: namespace.into(),
            attributes,
        }))
        .await?;

    transaction_context(res, ctx).await
}

pub async fn entity<'a>(
    ctx: &Context<'a>,
    external_id: String,
    namespace: Option<String>,
    attributes: Attributes,
) -> async_graphql::Result<Submission> {
    let api = ctx.data_unchecked::<ApiDispatch>();

    let namespace = namespace.unwrap_or_else(|| "default".to_owned());

    let res = api
        .dispatch(ApiCommand::Entity(EntityCommand::Create {
            external_id: external_id.into(),
            namespace: namespace.into(),
            attributes,
        }))
        .await?;

    transaction_context(res, ctx).await
}

pub async fn acted_on_behalf_of<'a>(
    ctx: &Context<'a>,
    namespace: Option<String>,
    responsible_id: AgentId,
    delegate_id: AgentId,
    activity_id: Option<ActivityId>,
    role: Option<Role>,
) -> async_graphql::Result<Submission> {
    let api = ctx.data_unchecked::<ApiDispatch>();

    let namespace = namespace.unwrap_or_else(|| "default".to_owned()).into();

    let res = api
        .dispatch(ApiCommand::Agent(AgentCommand::Delegate {
            id: responsible_id,
            delegate: delegate_id,
            activity: activity_id,
            namespace,
            role,
        }))
        .await?;

    transaction_context(res, ctx).await
}

pub async fn was_derived_from<'a>(
    ctx: &Context<'a>,
    namespace: Option<String>,
    generated_entity: EntityId,
    used_entity: EntityId,
) -> async_graphql::Result<Submission> {
    derivation(ctx, namespace, generated_entity, used_entity, None).await
}

pub async fn was_revision_of<'a>(
    ctx: &Context<'a>,
    namespace: Option<String>,
    generated_entity: EntityId,
    used_entity: EntityId,
) -> async_graphql::Result<Submission> {
    derivation(
        ctx,
        namespace,
        generated_entity,
        used_entity,
        Some(DerivationType::Revision),
    )
    .await
}
pub async fn had_primary_source<'a>(
    ctx: &Context<'a>,
    namespace: Option<String>,
    generated_entity: EntityId,
    used_entity: EntityId,
) -> async_graphql::Result<Submission> {
    derivation(
        ctx,
        namespace,
        generated_entity,
        used_entity,
        Some(DerivationType::PrimarySource),
    )
    .await
}
pub async fn was_quoted_from<'a>(
    ctx: &Context<'a>,
    namespace: Option<String>,
    generated_entity: EntityId,
    used_entity: EntityId,
) -> async_graphql::Result<Submission> {
    derivation(
        ctx,
        namespace,
        generated_entity,
        used_entity,
        Some(DerivationType::Quotation),
    )
    .await
}

pub async fn generate_key<'a>(
    ctx: &Context<'a>,
    id: AgentId,
    namespace: Option<String>,
) -> async_graphql::Result<Submission> {
    let api = ctx.data_unchecked::<ApiDispatch>();

    let namespace = namespace.unwrap_or_else(|| "default".to_owned()).into();

    let res = api
        .dispatch(ApiCommand::Agent(AgentCommand::RegisterKey {
            id,
            namespace,
            registration: KeyRegistration::Generate,
        }))
        .await?;

    transaction_context(res, ctx).await
}

pub async fn start_activity<'a>(
    ctx: &Context<'a>,
    id: ActivityId,
    namespace: Option<String>,
    agent: Option<AgentId>,
    time: Option<DateTime<Utc>>,
) -> async_graphql::Result<Submission> {
    let api = ctx.data_unchecked::<ApiDispatch>();

    let namespace = namespace.unwrap_or_else(|| "default".to_owned()).into();

    let res = api
        .dispatch(ApiCommand::Activity(ActivityCommand::Start {
            id,
            namespace,
            time,
            agent,
        }))
        .await?;

    transaction_context(res, ctx).await
}

pub async fn end_activity<'a>(
    ctx: &Context<'a>,
    id: ActivityId,
    namespace: Option<String>,
    agent: Option<AgentId>,
    time: Option<DateTime<Utc>>,
) -> async_graphql::Result<Submission> {
    let api = ctx.data_unchecked::<ApiDispatch>();

    let namespace = namespace.unwrap_or_else(|| "default".to_owned()).into();

    let res = api
        .dispatch(ApiCommand::Activity(ActivityCommand::End {
            id,
            namespace,
            time,
            agent,
        }))
        .await?;

    transaction_context(res, ctx).await
}

pub async fn instant_activity<'a>(
    ctx: &Context<'a>,
    id: ActivityId,
    namespace: Option<String>,
    agent: Option<AgentId>,
    time: Option<DateTime<Utc>>,
) -> async_graphql::Result<Submission> {
    let api = ctx.data_unchecked::<ApiDispatch>();

    let namespace = namespace.unwrap_or_else(|| "default".to_owned()).into();

    let res = api
        .dispatch(ApiCommand::Activity(ActivityCommand::Instant {
            id,
            namespace,
            time,
            agent,
        }))
        .await?;

    transaction_context(res, ctx).await
}

pub async fn was_associated_with<'a>(
    ctx: &Context<'a>,
    namespace: Option<String>,
    responsible: AgentId,
    activity: ActivityId,
    role: Option<Role>,
) -> async_graphql::Result<Submission> {
    let api = ctx.data_unchecked::<ApiDispatch>();

    let namespace = namespace.unwrap_or_else(|| "default".to_owned()).into();

    let res = api
        .dispatch(ApiCommand::Activity(ActivityCommand::Associate {
            id: activity,
            responsible,
            role,
            namespace,
        }))
        .await?;

    transaction_context(res, ctx).await
}

pub async fn used<'a>(
    ctx: &Context<'a>,
    activity: ActivityId,
    entity: EntityId,
    namespace: Option<String>,
) -> async_graphql::Result<Submission> {
    let api = ctx.data_unchecked::<ApiDispatch>();

    let namespace = namespace.unwrap_or_else(|| "default".to_owned()).into();

    let res = api
        .dispatch(ApiCommand::Activity(ActivityCommand::Use {
            id: entity,
            namespace,
            activity,
        }))
        .await?;

    transaction_context(res, ctx).await
}

pub async fn was_informed_by<'a>(
    ctx: &Context<'a>,
    activity: ActivityId,
    informing_activity: ActivityId,
    namespace: Option<String>,
) -> async_graphql::Result<Submission> {
    let api = ctx.data_unchecked::<ApiDispatch>();

    let namespace = namespace.unwrap_or_else(|| "default".to_owned()).into();

    let res = api
        .dispatch(ApiCommand::Activity(ActivityCommand::WasInformedBy {
            id: activity,
            namespace,
            informing_activity,
        }))
        .await?;

    transaction_context(res, ctx).await
}

pub async fn was_generated_by<'a>(
    ctx: &Context<'a>,
    activity: ActivityId,
    entity: EntityId,
    namespace: Option<String>,
) -> async_graphql::Result<Submission> {
    let api = ctx.data_unchecked::<ApiDispatch>();

    let namespace = namespace.unwrap_or_else(|| "default".to_owned()).into();

    let res = api
        .dispatch(ApiCommand::Activity(ActivityCommand::Generate {
            id: entity,
            namespace,
            activity,
        }))
        .await?;

    transaction_context(res, ctx).await
}

pub async fn has_attachment<'a>(
    ctx: &Context<'a>,
    entity: EntityId,
    namespace: Option<String>,
    attachment: Upload,
    agent: AgentId,
    locator: String,
) -> async_graphql::Result<Submission> {
    let api = ctx.data_unchecked::<ApiDispatch>();

    let namespace = namespace.unwrap_or_else(|| "default".to_owned()).into();

    let res = api
        .dispatch(ApiCommand::Entity(EntityCommand::Attach {
            id: entity,
            namespace,
            agent: Some(agent),
            file: PathOrFile::File(Arc::new(Box::pin(attachment.value(ctx)?.into_async_read()))),
            locator: Some(locator),
        }))
        .await?;

    transaction_context(res, ctx).await
}