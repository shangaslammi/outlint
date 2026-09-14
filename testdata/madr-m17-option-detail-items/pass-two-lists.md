# Choose a datastore

## Pros and Cons of the Options

### PostgreSQL

- Good, because it is mature

<!-- This transparent block keeps the two lists syntactically distinct. -->

- Neutral, because operations are familiar

<!--
Passes by design of the current schema language: the leading `block: any`
rule can absorb a list, and no block matcher expresses "any block except a
list". The first list above is therefore leading content and its items are
not classified. A complement matcher would let this fixture fail again.
-->
