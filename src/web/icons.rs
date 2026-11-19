//! The icon before a card's title, which stands for what the card holds
//! (`docs/specs/web/design_system.md`, Cards), drawn by the `card_icon`
//! macro.

use super::shell::Area;

#[derive(Clone, Copy)]
pub(super) enum CardIcon {
    /// What an area holds, by its icon in the navigation.
    Area(Area),
    /// A benchmark run's configurations.
    Configurations,
    /// A grid of passes, a task's under each configuration.
    Grid,
    /// A chart.
    Chart,
}
