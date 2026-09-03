//! YAML machinery shared by the two document paths that read YAML.
//!
//! Frontmatter and schema documents are read by separate readers, because a
//! schema needs source ranges where frontmatter needs body anchors, but what a
//! scalar means and how much a document may ask for are the same questions in
//! both. Those answers live here so that neither reader owns them on the
//! other's behalf.

mod scalar;

#[cfg(test)]
mod tests;

pub(crate) use scalar::{
    exact_yaml_scalar_to_json, parse_frontmatter_scalar, validate_yaml_container_tag,
    ExactYamlScalar, YamlValueError,
};

/// How deeply YAML collections may nest before Outlint refuses to read them.
///
/// Every tree over YAML in this crate is built and walked by recursion — the
/// frontmatter reader, the schema loader's reader, their conversions to JSON,
/// and the dropping of the JSON value itself — so nesting costs stack rather
/// than the heap the [node budget](EXACT_YAML_NODES_PER_EVENT) bounds. A
/// compact block sequence nests without indenting, so `- - - …` on one short
/// line reaches a depth no stack survives, and the parser's own `recursion
/// limit` counts flow nesting alone and never sees it. A fixed limit is the
/// right shape here where a size-scaled one is not: what a level costs is a
/// stack frame, which the input's size says nothing about.
///
/// The value is the recursion limit the discarded serde parsers enforced,
/// which both document paths had for free while they parsed through serde,
/// and serde_json's default nesting limit for the same purpose. Frontmatter
/// written to be read nests two or three deep and a schema a handful, so the
/// limit is an order of magnitude clear of any document meant for a reader,
/// and §1.6 requires at least half of it of any implementation.
pub(crate) const MAX_YAML_DEPTH: usize = 128;

/// A YAML document asked for more than one of this module's limits allows.
///
/// The refusal carries no words of its own: which limit was overrun is known
/// at the call that charged it, and each document path names the document it
/// was reading — frontmatter or schema — in its own vocabulary.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct YamlLimitExceeded;

/// How many nodes a YAML reader may build per parser event it has read.
///
/// An alias is one event that copies a whole subtree, so without a ceiling a
/// chain of them multiplies: fourteen lines of `a: &x [*w,*w,*w,*w]` name
/// hundreds of millions of nodes, which §1.6 lets an implementation refuse. The
/// factor
/// matches the one the discarded serde parse used to impose for free —
/// `yaml_serde` caps alias repetition at `events.len() * 100` — which is wide
/// enough that no document written to be read has ever met it.
pub(crate) const EXACT_YAML_NODES_PER_EVENT: usize = 100;

/// What a YAML reader has spent: parser events read, and nodes built.
///
/// The two together bound alias expansion. Events measure the input, since each
/// one needs source text of its own to exist, and nodes measure the tree the
/// input produces, alias copies included. Holding the second under a multiple
/// of the first bounds the memory a frontmatter block can ask for by its own
/// size, which is the property the removed serde parse had been supplying.
///
/// The count of events read *so far* stands in for the count of events in the
/// whole stream, so that nothing has to parse the block twice to know its size.
/// It never binds tighter than the material an alias could copy: an anchor
/// resolves only once its node has been parsed, so every event of that node is
/// already counted by the time an alias to it is read.
#[derive(Debug, Default)]
pub(crate) struct ExactYamlBudget {
    pub(crate) events: usize,
    pub(crate) nodes: usize,
}

impl ExactYamlBudget {
    /// Records `nodes` further nodes, refusing the ones that overrun the budget.
    ///
    /// Called before the nodes are built, so the refusal precedes the
    /// allocation rather than reporting it after the fact.
    pub(crate) fn spend(&mut self, nodes: usize) -> Result<(), YamlLimitExceeded> {
        self.nodes = self.nodes.saturating_add(nodes);
        if self.nodes > self.events.saturating_mul(EXACT_YAML_NODES_PER_EVENT) {
            return Err(YamlLimitExceeded);
        }
        Ok(())
    }
}

/// Opens `levels` further levels of nesting, refusing to pass
/// [`MAX_YAML_DEPTH`].
///
/// A collection opens one level, while an alias opens as many as the node it
/// copies reaches, which is why the count is a parameter rather than always
/// one. An event-counting scan cannot see the second kind: an alias is a
/// single event however deep the value it names, so nesting spliced in by an
/// alias is a depth only this bound sees. The bound lives beside the readers
/// in any case, because the recursion it guards is their own and a bound that
/// lives in a different function is one a later change can quietly remove.
pub(crate) fn deeper_yaml_nesting(depth: usize, levels: usize) -> Result<usize, YamlLimitExceeded> {
    let depth = depth.saturating_add(levels);
    if depth > MAX_YAML_DEPTH {
        return Err(YamlLimitExceeded);
    }
    Ok(depth)
}
