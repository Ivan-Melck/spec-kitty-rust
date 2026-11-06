//! DRG node adapter for the canonical mission-qualified template catalog.
//!
//! Mirrors `charter.offering.template_catalog.template_node` / `template_nodes`: each
//! discovered template becomes one typed [`DrgNode`] of [`NodeKind::Template`] whose URN
//! is the mission-qualified `template:<mission>/<name>` and whose label is the same
//! template id. Because the mission segment is part of the identity, the same file name
//! under two missions mints **two distinct** nodes.
//!
//! # Authorities this module reuses (it owns none of them)
//!
//! * Discovery, the mission-qualified identity and the `template:` URN spelling come from
//!   the ONE canonical catalog, `sk_charter::template_catalog`
//!   ([`TemplateRef`], [`template_urn`]). This module never walks a tier root, never
//!   re-derives an id and never re-splits `<mission>/<name>`.
//! * Node validity, the URN grammar and the prefix/kind agreement come from the ONE
//!   graph authority, [`DrgNode::new`]. This module adds no second validator and no
//!   parallel node model.
//! * The graph itself is not duplicated: nodes are returned as data for the existing
//!   merge/aggregation/validation layer to include, exactly like the reference, which
//!   "does not perform any merge itself".
//!
//! # Known gaps (explicit)
//!
//! The URN grammar is the reference's own `^[a-z_]+:[A-Za-z0-9_/.\-]+$`. A template file
//! whose name carries a character outside that class (for example a space) is a real
//! reference `ValueError` from `DRGNode` validation; here it is the typed
//! [`DoctrineError`] from `DrgNode::new`, and the caller keeps the discovery-level ref.
//! No lossy escaping of the file name is invented to make such a node mint.

use sk_charter::template_catalog::{TemplateRef, template_urn};

use crate::error::DoctrineError;
use crate::graph::{DrgNode, NodeKind};

/// Mint the typed DRG node of one discovered template.
///
/// The node carries kind [`NodeKind::Template`], the URN `template:<mission>/<name>` and
/// the mission-qualified template id as its label — the reference's
/// `DRGNode(urn=template_urn(ref.template_id), kind=NodeKind.TEMPLATE,
/// label=ref.template_id)`.
///
/// # Errors
///
/// [`DoctrineError::InvalidUrn`] / [`DoctrineError::UrnKindMismatch`] when the
/// canonical graph authority rejects the minted URN, which can only happen for a
/// template name outside the reference's own URN character class.
pub fn template_node(reference: &TemplateRef) -> Result<DrgNode, DoctrineError> {
    DrgNode::new(
        template_urn(&reference.template_id),
        NodeKind::Template,
        Some(reference.template_id.clone()),
        Vec::new(),
    )
}

/// Mint the typed DRG node of every discovered template, in catalog order.
///
/// # Errors
///
/// The first refusal from [`template_node`].
pub fn template_nodes(references: &[TemplateRef]) -> Result<Vec<DrgNode>, DoctrineError> {
    references.iter().map(template_node).collect()
}
