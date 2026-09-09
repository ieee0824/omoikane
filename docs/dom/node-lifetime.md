# DOM wrappers and document lifetime

Related: [Omoikane #620](https://github.com/ieee0824/omoikane/issues/620),
[Boa #77](https://github.com/ieee0824/boa/pull/77).

## Observable contract

Reloading or removing an iframe retires its browsing context, but JavaScript
references to its old Document and Nodes remain usable. Node identity, DOM
contents, attributes, parent/owner relationships, and wrapper expandos survive.
Mutations affect the old document only. `isConnected` still means connected to a
Document; it does not mean that the Document is active or rendered.

The native activity checks separately prevent old documents from creating iframe
contexts, fetching script resources, obtaining layout boxes, or receiving focus.
Realm identity checks cancel timers and reject network calls after retirement.
Queued resource tasks also check activity when they execute.

The existing `defaultView === null` behavior for retired documents is preserved.
This change does not implement the full browser WindowProxy/defaultView behavior
across navigation.

## Ownership

`src/js/node_lifetime.rs` groups canonical wrappers by `(owner Document, creating
Realm)`. Each group owns a native document record containing its DOM handles.
The same document record can be shared by groups from different Realms.

A bootstrap-private WeakMap associates each wrapper with a native lease object.
That lease traces its group object; the group traces all wrappers it has created.
These are collectible JavaScript cycles, not Rust `Rc` cycles. The bootstrap's
node-id index contains WeakRefs, and wrapper metadata uses WeakMaps.

HostState roots a group only while both its owner document and creating Realm are
active. Retirement removes both kinds of roots, including groups for parent DOM
wrappers created in an old child Realm. Explicit JS references can still keep
retired/inert groups alive. Retention is document-scoped: one retained wrapper
keeps its document, ancestors, and other known wrappers/expandos alive. There is
no generation-count limit or LRU eviction of referenced documents. Per-node
reclamation within a still-live document is outside this change.

Groups and leases use `GcRefCell` for native mutation. Each cell is traced through
its owning JS object, installing Boa's generational write barrier. Mutating a
promoted group must not lose newly created wrappers during a minor collection.
Adoption moves every Realm's alias to the new document group and updates its
lease; temporary explicit roots protect source groups while destinations allocate.

## Reclamation and caches

HostState keeps weak native document/node indices. When a JS group is collected,
its native document record can die. A later host checkpoint prunes expired
records, nodes, style resolvers, adopted-style snapshots, and document-write
parser state. Cache-owned DOM handles do not determine liveness, so style caches
cannot keep themselves alive indirectly. Inert and cloned documents use the same
mechanism as retired iframe documents.

Each Realm periodically removes dead node-id index entries and primitive metadata
sets. It asks the native registry for liveness instead of dereferencing all
WeakRefs, which would keep otherwise unreachable wrappers alive for the current
job. Retirement also triggers this sweep. Template contents' owner relationships
are private weak-key associations; slot bookkeeping uses WeakRefs.

Boa's shared prototype transition cache previously retained prototype objects as
strong map keys even when the transition shapes were dead. That kept discarded
Realms and their globals alive. The pinned dependency fix uses non-owning address
keys with weak shape targets. A live shape itself retains its actual prototype;
addresses are compared and hashed only, never dereferenced.

## Regression coverage

`src/js/node_lifetime_tests.rs` covers retained DOM mutation and expandos, 80
referenced generations followed by reclamation, inert/cloned-document style and
parser cleanup, adoption, template ownership, nested Realm cleanup, adopted
styles, inactive resources/timers/focus, and minor-GC write barriers. GC tests
explicitly end WeakRef keep-alive intervals before collection and inspect native
registry/cache counts; they do not use process RSS as a collection assertion.
