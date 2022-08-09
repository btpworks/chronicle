# Querying Provenance

Currently Chronicle has 4 root queries.

``` graphql
type Query {
    activityTimeline(activityTypes: [ActivityType!]!, forEntity: [EntityID!]!, from: DateTime, to: DateTime, namespace: ID, after: String, before: String, first: Int, last: Int): ActivityConnection!
    agentsByType(agentType: AgentType!, namespace: String, after: String, before: String, first: Int, last: Int): AgentConnection!
    agentById(id: AgentID!, namespace: String): Agent
    entityById(id: EntityID!, namespace: String): Entity
}
```

The majority of the work for provenance retrieval will be with the activityTimeline query.

Familiarizing yourself with GraphQL is necessary to make good use of Chronicle. Chronicle makes extensive use of [relay cursors](https://relay.dev/graphql/connections.htm) and [union types](https://www.apollographql.com/docs/apollo-server/schema/unions-interfaces/).


## activityTimeline

## agentsByType

## agentById

## entityById

