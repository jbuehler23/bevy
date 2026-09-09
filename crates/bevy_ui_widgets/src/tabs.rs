use accesskit::Role;
use bevy_a11y::AccessibilityNode;
use bevy_app::{App, Last, Plugin, PostUpdate};
use bevy_ecs::{
    change_detection::DetectChanges,
    component::Component,
    entity::Entity,
    event::EntityEvent,
    hierarchy::{ChildOf, Children},
    lifecycle::RemovedComponents,
    observer::On,
    query::{Added, Changed, Has, Or, With},
    reflect::{ReflectComponent, ReflectEvent},
    resource::Resource,
    schedule::IntoScheduleConfigs,
    system::{Commands, Query, Res, ResMut},
    template::FromTemplate,
};
use bevy_input::{
    keyboard::{KeyCode, KeyboardInput},
    ButtonState,
};
use bevy_input_focus::{
    tab_navigation::TabIndex, FocusCause, FocusedInput, InputFocus, InputFocusSystems,
    InputFocusVisible,
};
use bevy_picking::{
    events::{PointerCancel, PointerClick, PointerDrag, PointerDragEnd, PointerDragStart},
    hover::HoverMap,
    pointer::{PointerButton, PointerId},
};
use bevy_reflect::{prelude::ReflectDefault, Reflect};
use bevy_ui::{InteractionDisabled, Selectable, Selected, UiGlobalTransform};

use crate::ControlOrientation;

/// Determines whether moving keyboard focus also requests tab selection.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Reflect)]
#[reflect(Default, Clone, PartialEq)]
pub enum TabActivation {
    /// Enter or Space requests selection of the focused tab.
    #[default]
    Manual,
    /// Moving focus with a navigation key requests selection of the focused tab.
    Automatic,
}

/// Determines which drag gestures a [`TabList`] accepts.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Reflect)]
#[reflect(Default, Clone, PartialEq)]
pub enum TabDragMode {
    /// Dragging does not create tab gesture state.
    #[default]
    Disabled,
    /// Tabs may be reordered within this list.
    Reorder,
    /// Tabs may move between lists whose drag mode is also `External`.
    External,
}

/// Headless tab-strip behavior and policy.
///
/// Selection is stored separately in [`SelectedTab`]. User interaction emits
/// [`crate::ValueChange<Option<Entity>>`] from this entity and does not update that state unless
/// [`tablist_self_update`] is attached as an observer. Activating the already-selected tab does
/// not re-emit.
///
/// Only primary-button clicks change selection. [`InteractionDisabled`] on this entity disables
/// the whole strip; disabled strips and disabled tabs let pointer and keyboard events propagate
/// instead of consuming them.
#[derive(Component, Debug, Clone, Copy, PartialEq, Reflect)]
#[require(AccessibilityNode(accesskit::Node::new(Role::TabList)), SelectedTab)]
#[reflect(Component, Default, Clone, PartialEq)]
pub struct TabList {
    /// The axis used by arrow-key navigation.
    pub orientation: ControlOrientation,
    /// Whether keyboard navigation requests selection immediately.
    pub activation: TabActivation,
    /// The drag gestures accepted by this strip.
    pub drag: TabDragMode,
}

impl Default for TabList {
    fn default() -> Self {
        Self {
            orientation: ControlOrientation::Horizontal,
            activation: TabActivation::default(),
            drag: TabDragMode::default(),
        }
    }
}

/// The selected [`Tab`] within a [`TabList`].
///
/// The referenced entity must be an enabled direct child of the list. Missing, stale, disabled,
/// and unrelated entities are treated as no selection.
#[derive(Component, FromTemplate, Debug, Default, PartialEq, Eq, Reflect)]
#[reflect(Component, Default, PartialEq)]
pub struct SelectedTab(#[template(built_in)] pub Option<Entity>);

/// A headless tab header.
///
/// Tabs are focusable using a roving [`TabIndex`]. Their derived [`Selected`] state mirrors the
/// containing list's valid [`SelectedTab`] value.
#[derive(Component, Debug, Default, Clone, Copy, Reflect)]
#[require(
    AccessibilityNode(accesskit::Node::new(Role::Tab)),
    Selectable,
    TabIndex(-1)
)]
#[reflect(Component, Default, Clone)]
pub struct Tab;

/// Prevents a [`Tab`] from starting a drag without disabling focus or activation.
#[derive(Component, Debug, Default, Clone, Copy, Reflect)]
#[reflect(Component, Default, Clone)]
pub struct TabLocked;

/// Marks a tab whose drag has crossed the movement threshold.
#[derive(Component, Debug, Clone, Copy, PartialEq, Eq, Reflect)]
#[reflect(Component, Clone, PartialEq)]
pub struct TabDragging {
    /// The pointer controlling the drag.
    pub pointer_id: PointerId,
}

/// One pointer's proposed insertion point within a destination [`TabList`].
///
/// `index` is measured in destination order after removing `tab`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Reflect)]
#[reflect(Clone, PartialEq)]
pub struct TabInsertionPoint {
    /// The pointer controlling this preview.
    pub pointer_id: PointerId,
    /// The tab that would be inserted.
    pub tab: Entity,
    /// The proposed insertion index.
    pub index: usize,
}

/// The insertion points currently proposed on a destination [`TabList`].
///
/// The component exists only while at least one accepted drag targets the list. Each pointer has
/// at most one entry.
#[derive(Component, Debug, Default, Clone, PartialEq, Eq, Reflect)]
#[reflect(Component, Default, Clone, PartialEq)]
pub struct TabInsertionPreview {
    /// Proposed insertions keyed by each controlling pointer.
    pub entries: Vec<TabInsertionPoint>,
}

/// Proposes moving a tab without changing the entity hierarchy.
///
/// `index` is measured in destination order after removing `tab`.
#[derive(Copy, Clone, Debug, PartialEq, Eq, EntityEvent, Reflect)]
#[reflect(Event, Clone, PartialEq)]
pub struct TabMoved {
    /// The strip that currently contains the tab and receives this event.
    #[event_target]
    pub from_strip: Entity,
    /// The tab to move.
    pub tab: Entity,
    /// The proposed destination strip.
    pub to_strip: Entity,
    /// The proposed insertion index.
    pub index: usize,
}

/// A compatible tab-list destination accepted when a drag completes.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Reflect)]
#[reflect(Clone, PartialEq)]
pub struct TabDrop {
    /// The destination strip.
    pub destination: Entity,
    /// The insertion index in destination order after removing the dragged tab.
    pub index: usize,
}

/// The accepted lifecycle state of a tab drag.
#[derive(Clone, Debug, PartialEq, Eq, Reflect)]
#[reflect(Clone, PartialEq)]
pub enum TabDragPhase {
    /// The drag crossed the movement threshold.
    Started,
    /// The pointer was released after an accepted drag.
    Completed {
        /// The compatible destination and insertion index, if a tab list accepted the drop.
        drop: Option<TabDrop>,
    },
    /// The accepted drag was cancelled.
    Cancelled,
}

/// Reports accepted tab-drag start, completion, and cancellation.
#[derive(Clone, Debug, PartialEq, Eq, EntityEvent, Reflect)]
#[reflect(Event, Clone, PartialEq)]
pub struct TabDragLifecycle {
    /// The source strip that receives this event.
    #[event_target]
    pub source: Entity,
    /// The dragged tab.
    pub tab: Entity,
    /// The controlling pointer.
    pub pointer_id: PointerId,
    /// The lifecycle transition.
    pub phase: TabDragPhase,
}

const TAB_DRAG_THRESHOLD: f32 = 4.0;

#[derive(Resource, Default)]
struct TabDragGestures(Vec<TabDragGesture>);

struct TabDragGesture {
    pointer_id: PointerId,
    tab: Entity,
    source: Entity,
    accepted: bool,
    preview: Option<TabDrop>,
}

#[derive(Resource, Default)]
struct SuppressedTabClicks(Vec<(PointerId, Entity)>);

/// Plugin that registers tab-list observers and derived state.
pub struct TabPlugin;

impl Plugin for TabPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<TabDragGestures>()
            .init_resource::<SuppressedTabClicks>()
            .init_resource::<HoverMap>()
            .add_observer(tablist_on_click)
            .add_observer(tab_on_key_input)
            .add_observer(tab_on_drag_start)
            .add_observer(tab_on_drag)
            .add_observer(tab_on_drag_end)
            .add_observer(tab_on_drag_cancel)
            .add_observer(cancel_tab_drags_on_escape)
            .add_systems(
                PostUpdate,
                (
                    cleanup_orphaned_tab_drags,
                    sync_tab_insertion_previews,
                    update_tablist_derived_state,
                )
                    .chain()
                    .after(crate::MenuFocusSystem)
                    .before(InputFocusSystems::FocusChangeEvents),
            )
            .add_systems(Last, clear_suppressed_tab_clicks);
    }
}

/// Observer that applies tab selection requests to [`SelectedTab`].
pub fn tablist_self_update(
    change: On<crate::ValueChange<Option<Entity>>>,
    tablists: Query<(), With<TabList>>,
    mut commands: Commands,
) {
    if tablists.contains(change.source) {
        commands
            .entity(change.source)
            .insert(SelectedTab(change.value));
    }
}

fn tablist_on_click(
    mut click: On<PointerClick>,
    tablists: Query<(&SelectedTab, Has<InteractionDisabled>), With<TabList>>,
    tabs: Query<Has<InteractionDisabled>, With<Tab>>,
    parents: Query<&ChildOf>,
    mut suppressed_clicks: ResMut<SuppressedTabClicks>,
    mut commands: Commands,
) {
    if click.button != PointerButton::Primary {
        return;
    }
    let Ok((selection, list_disabled)) = tablists.get(click.entity) else {
        return;
    };

    let target = click.original_event_target();
    let tab = if tabs.contains(target) {
        Some(target)
    } else {
        parents
            .iter_ancestors(target)
            .take_while(|ancestor| *ancestor != click.entity)
            .find(|ancestor| tabs.contains(*ancestor))
    };
    let Some(tab) = tab else {
        return;
    };
    let Ok(parent) = parents.get(tab) else {
        return;
    };
    if parent.parent() != click.entity {
        return;
    }
    if list_disabled || tabs.get(tab).is_ok_and(|disabled| disabled) {
        return;
    }

    click.propagate(false);
    if let Some(index) = suppressed_clicks
        .0
        .iter()
        .position(|(pointer_id, suppressed_tab)| {
            *pointer_id == click.pointer.id && *suppressed_tab == tab
        })
    {
        suppressed_clicks.0.swap_remove(index);
        return;
    }
    if selection.0 != Some(tab) {
        commands.trigger(crate::ValueChange::<Option<Entity>> {
            source: click.entity,
            value: Some(tab),
            is_final: true,
        });
    }
}

fn tab_on_key_input(
    mut input: On<FocusedInput<KeyboardInput>>,
    tablists: Query<(&TabList, &SelectedTab, &Children, Has<InteractionDisabled>)>,
    tabs: Query<Has<InteractionDisabled>, With<Tab>>,
    parents: Query<&ChildOf>,
    mut focus: ResMut<InputFocus>,
    mut focus_visible: ResMut<InputFocusVisible>,
    mut commands: Commands,
) {
    if !tabs.contains(input.focused_entity) {
        return;
    }
    let Ok(parent) = parents.get(input.focused_entity) else {
        return;
    };
    let Ok((tablist, selection, children, list_disabled)) = tablists.get(parent.parent()) else {
        return;
    };
    if list_disabled {
        return;
    }
    let event = &input.input;
    if event.state != ButtonState::Pressed || event.repeat {
        return;
    }

    enum Navigation {
        Previous,
        Next,
        First,
        Last,
        Activate,
    }

    let navigation = match event.key_code {
        KeyCode::ArrowLeft if tablist.orientation == ControlOrientation::Horizontal => {
            Navigation::Previous
        }
        KeyCode::ArrowRight if tablist.orientation == ControlOrientation::Horizontal => {
            Navigation::Next
        }
        KeyCode::ArrowUp if tablist.orientation == ControlOrientation::Vertical => {
            Navigation::Previous
        }
        KeyCode::ArrowDown if tablist.orientation == ControlOrientation::Vertical => {
            Navigation::Next
        }
        KeyCode::Home => Navigation::First,
        KeyCode::End => Navigation::Last,
        KeyCode::Enter | KeyCode::Space => Navigation::Activate,
        _ => return,
    };

    if matches!(navigation, Navigation::Activate) {
        if tabs
            .get(input.focused_entity)
            .is_ok_and(|disabled| disabled)
        {
            return;
        }
        input.propagate(false);
        if selection.0 != Some(input.focused_entity) {
            commands.trigger(crate::ValueChange::<Option<Entity>> {
                source: parent.parent(),
                value: Some(input.focused_entity),
                is_final: true,
            });
        }
        return;
    }
    input.propagate(false);

    let enabled = children
        .iter()
        .copied()
        .filter(|child| tabs.get(*child).is_ok_and(|disabled| !disabled))
        .collect::<Vec<_>>();
    if enabled.is_empty() {
        return;
    }

    let current = enabled
        .iter()
        .position(|tab| *tab == input.focused_entity)
        .or_else(|| {
            selection
                .0
                .and_then(|selected| enabled.iter().position(|tab| *tab == selected))
        });
    let next_index = match navigation {
        Navigation::Previous => current
            .filter(|index| *index > 0)
            .map_or(enabled.len() - 1, |index| index - 1),
        Navigation::Next => current.map_or(0, |index| (index + 1) % enabled.len()),
        Navigation::First => 0,
        Navigation::Last => enabled.len() - 1,
        Navigation::Activate => unreachable!(),
    };
    let next = enabled[next_index];
    if focus.get() != Some(next) {
        focus.set(next, FocusCause::Navigated);
    }
    focus_visible.0 = true;
    if tablist.activation == TabActivation::Automatic && selection.0 != Some(next) {
        commands.trigger(crate::ValueChange::<Option<Entity>> {
            source: parent.parent(),
            value: Some(next),
            is_final: true,
        });
    }
}

fn tab_on_drag_start(
    mut event: On<PointerDragStart>,
    tabs: Query<(Has<InteractionDisabled>, Has<TabLocked>), With<Tab>>,
    tablists: Query<&TabList>,
    parents: Query<&ChildOf>,
    mut gestures: ResMut<TabDragGestures>,
) {
    if event.button != PointerButton::Primary {
        return;
    }
    let Ok((disabled, locked)) = tabs.get(event.entity) else {
        return;
    };
    let Ok(parent) = parents.get(event.entity) else {
        return;
    };
    let Ok(tablist) = tablists.get(parent.parent()) else {
        return;
    };
    if disabled
        || locked
        || tablist.drag == TabDragMode::Disabled
        || gestures
            .0
            .iter()
            .any(|gesture| gesture.pointer_id == event.pointer.id || gesture.tab == event.entity)
    {
        return;
    }

    event.propagate(false);
    gestures.0.push(TabDragGesture {
        pointer_id: event.pointer.id,
        tab: event.entity,
        source: parent.parent(),
        accepted: false,
        preview: None,
    });
}

fn tab_on_drag(
    mut event: On<PointerDrag>,
    mut gestures: ResMut<TabDragGestures>,
    tablists: Query<&TabList>,
    children: Query<&Children>,
    tabs: Query<(), With<Tab>>,
    transforms: Query<&UiGlobalTransform>,
    parents: Query<&ChildOf>,
    hover_map: Res<HoverMap>,
    mut commands: Commands,
) {
    let Some(gesture) = gestures
        .0
        .iter_mut()
        .find(|gesture| gesture.pointer_id == event.pointer.id && gesture.tab == event.entity)
    else {
        return;
    };
    event.propagate(false);
    if !gesture.accepted
        && event.distance.length_squared() < TAB_DRAG_THRESHOLD * TAB_DRAG_THRESHOLD
    {
        return;
    }

    if !gesture.accepted {
        gesture.accepted = true;
        commands.entity(gesture.tab).insert(TabDragging {
            pointer_id: gesture.pointer_id,
        });
        commands.trigger(TabDragLifecycle {
            source: gesture.source,
            tab: gesture.tab,
            pointer_id: gesture.pointer_id,
            phase: TabDragPhase::Started,
        });
    }

    let destination = resolve_tab_destination(
        gesture.source,
        gesture.pointer_id,
        &hover_map,
        &tablists,
        &parents,
    );
    let preview = destination.and_then(|destination| {
        tab_insertion_index(
            destination,
            gesture.tab,
            event.pointer.position,
            &tablists,
            &children,
            &tabs,
            &transforms,
        )
        .map(|index| (destination, index))
    });

    gesture.preview = preview.map(|(destination, index)| TabDrop { destination, index });
}

fn resolve_tab_destination(
    source: Entity,
    pointer_id: PointerId,
    hover_map: &HoverMap,
    tablists: &Query<&TabList>,
    parents: &Query<&ChildOf>,
) -> Option<Entity> {
    let source_mode = tablists.get(source).ok()?.drag;
    let hovered = hover_map.get(&pointer_id)?;
    hovered
        .iter()
        .filter_map(|(entity, hit)| {
            let destination = if tablists.contains(*entity) {
                Some(*entity)
            } else {
                parents
                    .iter_ancestors(*entity)
                    .find(|ancestor| tablists.contains(*ancestor))
            }?;
            let destination_mode = tablists.get(destination).ok()?.drag;
            let compatible = match source_mode {
                TabDragMode::Disabled => false,
                TabDragMode::Reorder => destination == source,
                TabDragMode::External => destination_mode == TabDragMode::External,
            };
            compatible.then_some((destination, hit.depth))
        })
        .min_by(|left, right| left.1.total_cmp(&right.1))
        .map(|(destination, _)| destination)
}

fn tab_insertion_index(
    destination: Entity,
    dragged_tab: Entity,
    pointer_position: bevy_math::Vec2,
    tablists: &Query<&TabList>,
    children: &Query<&Children>,
    tabs: &Query<(), With<Tab>>,
    transforms: &Query<&UiGlobalTransform>,
) -> Option<usize> {
    let tablist = tablists.get(destination).ok()?;
    let position = match tablist.orientation {
        ControlOrientation::Horizontal => pointer_position.x,
        ControlOrientation::Vertical => pointer_position.y,
    };
    let mut index = 0;
    if let Ok(children) = children.get(destination) {
        for child in children.iter().copied() {
            if child == dragged_tab || !tabs.contains(child) {
                continue;
            }
            let transform = transforms.get(child).ok()?;
            let center = match tablist.orientation {
                ControlOrientation::Horizontal => transform.translation.x,
                ControlOrientation::Vertical => transform.translation.y,
            };
            if position < center {
                return Some(index);
            }
            index += 1;
        }
    }
    Some(index)
}

fn tab_on_drag_end(
    mut event: On<PointerDragEnd>,
    mut gestures: ResMut<TabDragGestures>,
    tablists: Query<&TabList>,
    children: Query<&Children>,
    tabs: Query<(), With<Tab>>,
    transforms: Query<&UiGlobalTransform>,
    parents: Query<&ChildOf>,
    hover_map: Res<HoverMap>,
    mut suppressed_clicks: ResMut<SuppressedTabClicks>,
    mut commands: Commands,
) {
    let Some(index) = gestures
        .0
        .iter()
        .position(|gesture| gesture.pointer_id == event.pointer.id && gesture.tab == event.entity)
    else {
        return;
    };
    event.propagate(false);
    let gesture = gestures.0.swap_remove(index);
    if !gesture.accepted {
        return;
    }

    let destination = resolve_tab_destination(
        gesture.source,
        gesture.pointer_id,
        &hover_map,
        &tablists,
        &parents,
    );
    let insertion = destination.and_then(|destination| {
        tab_insertion_index(
            destination,
            gesture.tab,
            event.pointer.position,
            &tablists,
            &children,
            &tabs,
            &transforms,
        )
        .map(|index| (destination, index))
    });
    commands.entity(gesture.tab).remove::<TabDragging>();
    suppressed_clicks.0.push((gesture.pointer_id, gesture.tab));

    if let Some((destination, index)) = insertion {
        commands.trigger(TabMoved {
            from_strip: gesture.source,
            tab: gesture.tab,
            to_strip: destination,
            index,
        });
    }
    commands.trigger(TabDragLifecycle {
        source: gesture.source,
        tab: gesture.tab,
        pointer_id: gesture.pointer_id,
        phase: TabDragPhase::Completed {
            drop: insertion.map(|(destination, index)| TabDrop { destination, index }),
        },
    });
}

fn finish_tab_drag_cancellation(gesture: TabDragGesture, commands: &mut Commands) {
    if let Ok(mut tab) = commands.get_entity(gesture.tab) {
        tab.remove::<TabDragging>();
    }
    if gesture.accepted {
        commands.trigger(TabDragLifecycle {
            source: gesture.source,
            tab: gesture.tab,
            pointer_id: gesture.pointer_id,
            phase: TabDragPhase::Cancelled,
        });
    }
}

fn tab_on_drag_cancel(
    mut event: On<PointerCancel>,
    mut gestures: ResMut<TabDragGestures>,
    mut commands: Commands,
) {
    let Some(index) = gestures
        .0
        .iter()
        .position(|gesture| gesture.pointer_id == event.pointer.id && gesture.tab == event.entity)
    else {
        return;
    };
    event.propagate(false);
    let gesture = gestures.0.swap_remove(index);
    finish_tab_drag_cancellation(gesture, &mut commands);
}

fn cancel_tab_drags_on_escape(
    mut event: On<FocusedInput<KeyboardInput>>,
    mut gestures: ResMut<TabDragGestures>,
    mut commands: Commands,
) {
    if event.input.state != ButtonState::Pressed
        || event.input.repeat
        || event.input.key_code != KeyCode::Escape
        || gestures.0.is_empty()
    {
        return;
    }
    event.propagate(false);
    for gesture in core::mem::take(&mut gestures.0) {
        finish_tab_drag_cancellation(gesture, &mut commands);
    }
}

fn cleanup_orphaned_tab_drags(
    mut gestures: ResMut<TabDragGestures>,
    tabs: Query<(), With<Tab>>,
    tablists: Query<(), With<TabList>>,
    mut commands: Commands,
) {
    let mut index = 0;
    while index < gestures.0.len() {
        if tabs.contains(gestures.0[index].tab) {
            index += 1;
            continue;
        }
        let gesture = gestures.0.swap_remove(index);
        if gesture.accepted && tablists.contains(gesture.source) {
            commands.trigger(TabDragLifecycle {
                source: gesture.source,
                tab: gesture.tab,
                pointer_id: gesture.pointer_id,
                phase: TabDragPhase::Cancelled,
            });
        }
    }
}

fn sync_tab_insertion_previews(
    gestures: Res<TabDragGestures>,
    tablists: Query<(), With<TabList>>,
    mut previews: Query<(Entity, &mut TabInsertionPreview)>,
    mut commands: Commands,
) {
    for (destination, mut preview) in &mut previews {
        let entries = gestures
            .0
            .iter()
            .filter_map(|gesture| {
                gesture
                    .preview
                    .filter(|drop| drop.destination == destination)
                    .map(|TabDrop { index, .. }| TabInsertionPoint {
                        pointer_id: gesture.pointer_id,
                        tab: gesture.tab,
                        index,
                    })
            })
            .collect::<Vec<_>>();
        if entries.is_empty() {
            commands.entity(destination).remove::<TabInsertionPreview>();
        } else if preview.entries != entries {
            preview.entries = entries;
        }
    }

    for (gesture_index, gesture) in gestures.0.iter().enumerate() {
        let Some(drop) = gesture.preview else {
            continue;
        };
        if !tablists.contains(drop.destination) {
            continue;
        }
        let is_first_for_destination = gestures.0[..gesture_index].iter().all(|previous| {
            previous
                .preview
                .is_none_or(|preview| preview.destination != drop.destination)
        });
        if is_first_for_destination && !previews.contains(drop.destination) {
            let entries = gestures
                .0
                .iter()
                .filter_map(|gesture| {
                    gesture
                        .preview
                        .filter(|preview| preview.destination == drop.destination)
                        .map(|TabDrop { index, .. }| TabInsertionPoint {
                            pointer_id: gesture.pointer_id,
                            tab: gesture.tab,
                            index,
                        })
                })
                .collect();
            commands
                .entity(drop.destination)
                .insert(TabInsertionPreview { entries });
        }
    }
}

fn clear_suppressed_tab_clicks(mut suppressed_clicks: ResMut<SuppressedTabClicks>) {
    suppressed_clicks.0.clear();
}

/// Derives per-tab state from each [`TabList`]'s [`SelectedTab`] and the current keyboard focus:
///
/// - [`Selected`] markers mirror a validated `SelectedTab` (the referenced entity must be an
///   enabled direct child; anything else counts as no selection).
/// - [`TabIndex`] follows the roving-tabindex pattern: one tab per list is focusable (the
///   focused tab, else the selected tab, else the first enabled tab), so Tab/Shift+Tab skip
///   the strip while arrow keys move within it.
///
/// Runs in `PostUpdate`, early-returning unless selection, children, focus, or disabled state
/// changed.
fn update_tablist_derived_state(
    tablists: Query<(&SelectedTab, &Children), With<TabList>>,
    tabs: Query<(Has<InteractionDisabled>, Has<Selected>, &TabIndex), With<Tab>>,
    focus: Option<Res<InputFocus>>,
    changed_tablists: Query<(), (With<TabList>, Or<(Changed<SelectedTab>, Changed<Children>)>)>,
    changed_tabs: Query<(), (With<Tab>, Or<(Added<Tab>, Added<InteractionDisabled>)>)>,
    mut removed_disabled: RemovedComponents<InteractionDisabled>,
    mut commands: Commands,
) {
    let focus_changed = focus.as_ref().is_some_and(DetectChanges::is_changed);
    let disabled_removed = !removed_disabled.is_empty();
    removed_disabled.clear();
    if !focus_changed && !disabled_removed && changed_tablists.is_empty() && changed_tabs.is_empty()
    {
        return;
    }

    for (selection, children) in tablists.iter() {
        let selected = selection.0.filter(|entity| {
            children.contains(entity) && tabs.get(*entity).is_ok_and(|(disabled, _, _)| !disabled)
        });
        let focused = focus
            .as_ref()
            .and_then(|focus| focus.get())
            .filter(|entity| {
                children.contains(entity)
                    && tabs.get(*entity).is_ok_and(|(disabled, _, _)| !disabled)
            });
        let roving = focused.or(selected).or_else(|| {
            children
                .iter()
                .find(|child| tabs.get(**child).is_ok_and(|(disabled, _, _)| !disabled))
                .copied()
        });

        for child in children.iter() {
            let Ok((_, is_selected, tab_index)) = tabs.get(*child) else {
                continue;
            };
            let should_select = selected == Some(*child);
            if should_select && !is_selected {
                commands.entity(*child).insert(Selected);
            } else if !should_select && is_selected {
                commands.entity(*child).remove::<Selected>();
            }

            let desired_index = if roving == Some(*child) { 0 } else { -1 };
            if tab_index.0 != desired_index {
                commands.entity(*child).insert(TabIndex(desired_index));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bevy_ecs::{hierarchy::ChildOf, observer::On, resource::Resource, system::ResMut};
    use bevy_input::{keyboard::Key, InputPlugin};
    use bevy_input_focus::{FocusCause, InputDispatchPlugin, InputFocusPlugin};
    use bevy_math::Vec2;
    use bevy_picking::{
        backend::HitData,
        events::{Pointer, PointerCancel, PointerDrag, PointerDragEnd, PointerDragStart},
        hover::HoverMap,
        pointer::{Location, PointerButton, PointerId},
    };
    use bevy_ui::{ComputedNode, UiGlobalTransform};
    use bevy_window::{PrimaryWindow, Window, WindowRef};

    #[derive(Resource, Default)]
    struct SelectionRequests(Vec<(Entity, Option<Entity>)>);

    #[derive(Resource, Default)]
    struct DragLifecycleLog(Vec<TabDragLifecycle>);

    #[derive(Resource, Default)]
    struct TabMoveLog(Vec<TabMoved>);

    fn tab_app() -> (App, Entity) {
        let mut app = App::new();
        app.add_plugins((
            InputPlugin,
            InputFocusPlugin,
            InputDispatchPlugin,
            TabPlugin,
        ))
        .init_resource::<SelectionRequests>()
        .init_resource::<DragLifecycleLog>()
        .init_resource::<TabMoveLog>()
        .init_resource::<HoverMap>()
        .add_observer(
            |change: On<crate::ValueChange<Option<Entity>>>,
             mut requests: ResMut<SelectionRequests>| {
                requests.0.push((change.source, change.value));
            },
        )
        .add_observer(
            |event: On<TabDragLifecycle>, mut log: ResMut<DragLifecycleLog>| {
                log.0.push(event.event().clone());
            },
        )
        .add_observer(|event: On<TabMoved>, mut log: ResMut<TabMoveLog>| {
            log.0.push(*event.event());
        });
        let window = app
            .world_mut()
            .spawn((Window::default(), PrimaryWindow))
            .id();
        app.update();
        (app, window)
    }

    fn press_key(app: &mut App, key_code: KeyCode, window: Entity) {
        let logical_key = match key_code {
            KeyCode::ArrowLeft => Key::ArrowLeft,
            KeyCode::ArrowRight => Key::ArrowRight,
            KeyCode::ArrowUp => Key::ArrowUp,
            KeyCode::ArrowDown => Key::ArrowDown,
            KeyCode::Home => Key::Home,
            KeyCode::End => Key::End,
            KeyCode::Enter => Key::Enter,
            KeyCode::Space => Key::Space,
            KeyCode::Escape => Key::Escape,
            _ => Key::Unidentified(bevy_input::keyboard::NativeKey::Unidentified),
        };
        app.world_mut().write_message(KeyboardInput {
            key_code,
            logical_key,
            state: ButtonState::Pressed,
            text: None,
            repeat: false,
            window,
        });
        app.update();
    }

    fn click(app: &mut App, target: Entity, window: Entity) {
        click_with_button(app, target, window, PointerButton::Primary);
    }

    fn window_location(window: Entity) -> Location {
        window_location_at(window, Vec2::ZERO)
    }

    fn window_location_at(window: Entity, position: Vec2) -> Location {
        Location {
            target: bevy_camera::NormalizedRenderTarget::Window(
                WindowRef::Entity(window).normalize(Some(window)).unwrap(),
            ),
            position,
        }
    }

    fn click_with_button(app: &mut App, target: Entity, window: Entity, button: PointerButton) {
        app.world_mut().trigger(PointerClick {
            entity: target,
            pointer: Pointer::new(PointerId::Mouse, window_location(window)),
            button,
            hit: HitData::new(window, 0.0, None, None),
            duration: core::time::Duration::from_millis(10),
            count: 1,
        });
        app.update();
    }

    fn start_drag(app: &mut App, target: Entity, window: Entity, pointer_id: PointerId) {
        app.world_mut().trigger(PointerDragStart {
            entity: target,
            pointer: Pointer::new(pointer_id, window_location(window)),
            button: PointerButton::Primary,
            hit: HitData::new(window, 0.0, None, None),
        });
        app.update();
    }

    fn drag(app: &mut App, target: Entity, window: Entity, pointer_id: PointerId, distance: Vec2) {
        drag_at(app, target, window, pointer_id, distance, Vec2::ZERO);
    }

    fn drag_at(
        app: &mut App,
        target: Entity,
        window: Entity,
        pointer_id: PointerId,
        distance: Vec2,
        position: Vec2,
    ) {
        app.world_mut().trigger(PointerDrag {
            entity: target,
            pointer: Pointer::new(pointer_id, window_location_at(window, position)),
            button: PointerButton::Primary,
            distance,
            delta: distance,
        });
        app.update();
    }

    fn end_drag_at(
        app: &mut App,
        target: Entity,
        window: Entity,
        pointer_id: PointerId,
        distance: Vec2,
        position: Vec2,
    ) {
        end_drag_at_without_update(app, target, window, pointer_id, distance, position);
        app.update();
    }

    fn end_drag_at_without_update(
        app: &mut App,
        target: Entity,
        window: Entity,
        pointer_id: PointerId,
        distance: Vec2,
        position: Vec2,
    ) {
        app.world_mut().trigger(PointerDragEnd {
            entity: target,
            pointer: Pointer::new(pointer_id, window_location_at(window, position)),
            button: PointerButton::Primary,
            distance,
        });
    }

    fn cancel_drag(app: &mut App, target: Entity, window: Entity, pointer_id: PointerId) {
        app.world_mut().trigger(PointerCancel {
            entity: target,
            pointer: Pointer::new(pointer_id, window_location(window)),
            hit: HitData::new(window, 0.0, None, None),
        });
        app.update();
    }

    #[test]
    fn clicking_enabled_tab_requests_selection_without_mutating_controlled_state() {
        let (mut app, window) = tab_app();
        let list = app
            .world_mut()
            .spawn((TabList::default(), ChildOf(window)))
            .id();
        let first = app.world_mut().spawn((Tab, ChildOf(list))).id();
        let second = app.world_mut().spawn((Tab, ChildOf(list))).id();
        app.world_mut()
            .entity_mut(list)
            .insert(SelectedTab(Some(first)));
        app.update();

        click(&mut app, second, window);

        assert_eq!(
            app.world().resource::<SelectionRequests>().0,
            [(list, Some(second))]
        );
        assert_eq!(
            app.world().entity(list).get::<SelectedTab>(),
            Some(&SelectedTab(Some(first)))
        );
    }

    #[test]
    fn self_update_observer_applies_selection_request() {
        let (mut app, window) = tab_app();
        let list = app
            .world_mut()
            .spawn((TabList::default(), ChildOf(window)))
            .observe(tablist_self_update)
            .id();
        let tab = app.world_mut().spawn((Tab, ChildOf(list))).id();
        app.update();

        click(&mut app, tab, window);

        assert_eq!(
            app.world().entity(list).get::<SelectedTab>(),
            Some(&SelectedTab(Some(tab)))
        );
    }

    #[test]
    fn valid_selection_derives_selected_state_and_roving_entry() {
        let (mut app, window) = tab_app();
        let list = app
            .world_mut()
            .spawn((TabList::default(), ChildOf(window)))
            .id();
        let first = app.world_mut().spawn((Tab, ChildOf(list))).id();
        let second = app.world_mut().spawn((Tab, ChildOf(list))).id();
        app.world_mut()
            .entity_mut(list)
            .insert(SelectedTab(Some(second)));

        app.update();

        assert!(!app.world().entity(first).contains::<Selected>());
        assert!(app.world().entity(second).contains::<Selected>());
        assert_eq!(
            app.world().entity(first).get::<TabIndex>(),
            Some(&TabIndex(-1))
        );
        assert_eq!(
            app.world().entity(second).get::<TabIndex>(),
            Some(&TabIndex(0))
        );
    }

    #[test]
    fn invalid_selection_clears_selected_state_and_uses_first_enabled_roving_entry() {
        let (mut app, window) = tab_app();
        let stale = app.world_mut().spawn_empty().id();
        let list = app
            .world_mut()
            .spawn((
                TabList::default(),
                SelectedTab(Some(stale)),
                ChildOf(window),
            ))
            .id();
        let disabled = app
            .world_mut()
            .spawn((Tab, Selected, InteractionDisabled, ChildOf(list)))
            .id();
        let enabled = app.world_mut().spawn((Tab, ChildOf(list))).id();

        app.update();

        assert!(!app.world().entity(disabled).contains::<Selected>());
        assert!(!app.world().entity(enabled).contains::<Selected>());
        assert_eq!(
            app.world().entity(disabled).get::<TabIndex>(),
            Some(&TabIndex(-1))
        );
        assert_eq!(
            app.world().entity(enabled).get::<TabIndex>(),
            Some(&TabIndex(0))
        );
    }

    #[test]
    fn horizontal_manual_arrow_navigation_wraps_and_skips_disabled_tabs() {
        use bevy_input::keyboard::KeyCode;

        let (mut app, window) = tab_app();
        let list = app
            .world_mut()
            .spawn((TabList::default(), ChildOf(window)))
            .id();
        let first = app.world_mut().spawn((Tab, ChildOf(list))).id();
        app.world_mut()
            .spawn((Tab, InteractionDisabled, ChildOf(list)));
        let last = app.world_mut().spawn((Tab, ChildOf(list))).id();
        app.world_mut()
            .entity_mut(list)
            .insert(SelectedTab(Some(first)));
        app.world_mut()
            .resource_mut::<InputFocus>()
            .set(first, FocusCause::Navigated);
        app.update();

        press_key(&mut app, KeyCode::ArrowRight, window);
        assert_eq!(app.world().resource::<InputFocus>().get(), Some(last));

        press_key(&mut app, KeyCode::ArrowRight, window);
        assert_eq!(app.world().resource::<InputFocus>().get(), Some(first));

        press_key(&mut app, KeyCode::ArrowLeft, window);
        assert_eq!(app.world().resource::<InputFocus>().get(), Some(last));
        assert!(
            app.world().resource::<SelectionRequests>().0.is_empty(),
            "manual navigation must not request selection"
        );
    }

    #[test]
    fn automatic_navigation_requests_selection_when_focus_moves() {
        let (mut app, window) = tab_app();
        let list = app
            .world_mut()
            .spawn((
                TabList {
                    activation: TabActivation::Automatic,
                    ..Default::default()
                },
                ChildOf(window),
            ))
            .id();
        let first = app.world_mut().spawn((Tab, ChildOf(list))).id();
        let second = app.world_mut().spawn((Tab, ChildOf(list))).id();
        app.world_mut()
            .entity_mut(list)
            .insert(SelectedTab(Some(first)));
        app.world_mut()
            .resource_mut::<InputFocus>()
            .set(first, FocusCause::Navigated);
        app.update();

        press_key(&mut app, KeyCode::ArrowRight, window);

        assert_eq!(app.world().resource::<InputFocus>().get(), Some(second));
        assert_eq!(
            app.world().resource::<SelectionRequests>().0,
            [(list, Some(second))]
        );
        assert_eq!(
            app.world().entity(list).get::<SelectedTab>(),
            Some(&SelectedTab(Some(first))),
            "automatic activation remains a controlled selection request"
        );
    }

    #[test]
    fn manual_tabs_request_selection_on_enter_and_space() {
        let (mut app, window) = tab_app();
        let list = app
            .world_mut()
            .spawn((TabList::default(), ChildOf(window)))
            .id();
        let tab = app.world_mut().spawn((Tab, ChildOf(list))).id();
        app.world_mut()
            .resource_mut::<InputFocus>()
            .set(tab, FocusCause::Navigated);
        app.update();

        press_key(&mut app, KeyCode::Enter, window);
        press_key(&mut app, KeyCode::Space, window);

        assert_eq!(
            app.world().resource::<SelectionRequests>().0,
            [(list, Some(tab)), (list, Some(tab))]
        );
    }

    #[test]
    fn vertical_tabs_use_up_and_down_arrows_only() {
        let (mut app, window) = tab_app();
        let list = app
            .world_mut()
            .spawn((
                TabList {
                    orientation: ControlOrientation::Vertical,
                    ..Default::default()
                },
                ChildOf(window),
            ))
            .id();
        let first = app.world_mut().spawn((Tab, ChildOf(list))).id();
        let second = app.world_mut().spawn((Tab, ChildOf(list))).id();
        app.world_mut()
            .resource_mut::<InputFocus>()
            .set(first, FocusCause::Navigated);
        app.update();

        press_key(&mut app, KeyCode::ArrowRight, window);
        assert_eq!(app.world().resource::<InputFocus>().get(), Some(first));

        press_key(&mut app, KeyCode::ArrowDown, window);
        assert_eq!(app.world().resource::<InputFocus>().get(), Some(second));

        press_key(&mut app, KeyCode::ArrowUp, window);
        assert_eq!(app.world().resource::<InputFocus>().get(), Some(first));
    }

    #[test]
    fn home_and_end_focus_first_and_last_enabled_tabs() {
        let (mut app, window) = tab_app();
        let list = app
            .world_mut()
            .spawn((TabList::default(), ChildOf(window)))
            .id();
        app.world_mut()
            .spawn((Tab, InteractionDisabled, ChildOf(list)));
        let first_enabled = app.world_mut().spawn((Tab, ChildOf(list))).id();
        let last_enabled = app.world_mut().spawn((Tab, ChildOf(list))).id();
        app.world_mut()
            .spawn((Tab, InteractionDisabled, ChildOf(list)));
        app.world_mut()
            .resource_mut::<InputFocus>()
            .set(first_enabled, FocusCause::Navigated);
        app.update();

        press_key(&mut app, KeyCode::End, window);
        assert_eq!(
            app.world().resource::<InputFocus>().get(),
            Some(last_enabled)
        );

        press_key(&mut app, KeyCode::Home, window);
        assert_eq!(
            app.world().resource::<InputFocus>().get(),
            Some(first_enabled)
        );
    }

    #[test]
    fn focused_tab_is_the_roving_entry_during_manual_navigation() {
        let (mut app, window) = tab_app();
        let list = app
            .world_mut()
            .spawn((TabList::default(), ChildOf(window)))
            .id();
        let selected = app.world_mut().spawn((Tab, ChildOf(list))).id();
        let focused = app.world_mut().spawn((Tab, ChildOf(list))).id();
        app.world_mut()
            .entity_mut(list)
            .insert(SelectedTab(Some(selected)));
        app.world_mut()
            .resource_mut::<InputFocus>()
            .set(focused, FocusCause::Navigated);

        app.update();

        assert_eq!(
            app.world().entity(selected).get::<TabIndex>(),
            Some(&TabIndex(-1))
        );
        assert_eq!(
            app.world().entity(focused).get::<TabIndex>(),
            Some(&TabIndex(0))
        );
        assert!(app.world().entity(selected).contains::<Selected>());
        assert!(!app.world().entity(focused).contains::<Selected>());
    }

    #[test]
    fn tablist_and_tab_install_accessibility_semantics() {
        let (mut app, window) = tab_app();
        let list = app
            .world_mut()
            .spawn((TabList::default(), ChildOf(window)))
            .id();
        let tab = app.world_mut().spawn((Tab, ChildOf(list))).id();
        app.update();

        let list_node = app.world().entity(list).get::<AccessibilityNode>().unwrap();
        let tab_node = app.world().entity(tab).get::<AccessibilityNode>().unwrap();
        assert_eq!(list_node.role(), Role::TabList);
        assert_eq!(tab_node.role(), Role::Tab);
        assert!(app.world().entity(tab).contains::<Selectable>());
    }

    #[test]
    fn disabled_tab_does_not_request_selection() {
        let (mut app, window) = tab_app();
        let list = app
            .world_mut()
            .spawn((TabList::default(), ChildOf(window)))
            .id();
        let disabled = app
            .world_mut()
            .spawn((Tab, InteractionDisabled, ChildOf(list)))
            .id();
        app.update();

        click(&mut app, disabled, window);

        assert!(app.world().resource::<SelectionRequests>().0.is_empty());
    }

    #[test]
    fn secondary_click_does_not_change_selection() {
        let (mut app, window) = tab_app();
        let list = app
            .world_mut()
            .spawn((TabList::default(), ChildOf(window)))
            .id();
        let tab = app.world_mut().spawn((Tab, ChildOf(list))).id();
        app.update();

        click_with_button(&mut app, tab, window, PointerButton::Secondary);
        click_with_button(&mut app, tab, window, PointerButton::Middle);

        assert!(app.world().resource::<SelectionRequests>().0.is_empty());
    }

    #[test]
    fn activating_the_selected_tab_does_not_reemit() {
        let (mut app, window) = tab_app();
        let list = app
            .world_mut()
            .spawn((TabList::default(), ChildOf(window)))
            .observe(tablist_self_update)
            .id();
        let tab = app.world_mut().spawn((Tab, ChildOf(list))).id();
        app.update();

        click(&mut app, tab, window);
        click(&mut app, tab, window);

        assert_eq!(
            app.world().resource::<SelectionRequests>().0,
            [(list, Some(tab))]
        );
        assert_eq!(
            app.world().entity(list).get::<SelectedTab>(),
            Some(&SelectedTab(Some(tab)))
        );
    }

    #[test]
    fn disabled_tablist_ignores_clicks_and_keys() {
        let (mut app, window) = tab_app();
        let list = app
            .world_mut()
            .spawn((TabList::default(), InteractionDisabled, ChildOf(window)))
            .id();
        let first = app.world_mut().spawn((Tab, ChildOf(list))).id();
        let second = app.world_mut().spawn((Tab, ChildOf(list))).id();
        app.world_mut()
            .resource_mut::<InputFocus>()
            .set(first, FocusCause::Navigated);
        app.update();

        click(&mut app, second, window);
        press_key(&mut app, KeyCode::ArrowRight, window);
        press_key(&mut app, KeyCode::Enter, window);

        assert!(app.world().resource::<SelectionRequests>().0.is_empty());
        assert_eq!(app.world().resource::<InputFocus>().get(), Some(first));
    }

    #[test]
    fn tab_drag_is_accepted_only_after_crossing_movement_threshold() {
        let (mut app, window) = tab_app();
        let list = app
            .world_mut()
            .spawn((
                TabList {
                    drag: TabDragMode::Reorder,
                    ..Default::default()
                },
                ChildOf(window),
            ))
            .id();
        let tab = app.world_mut().spawn((Tab, ChildOf(list))).id();
        app.update();

        start_drag(&mut app, tab, window, PointerId::Mouse);
        drag(&mut app, tab, window, PointerId::Mouse, Vec2::splat(2.0));
        assert!(app.world().resource::<DragLifecycleLog>().0.is_empty());
        assert!(!app.world().entity(tab).contains::<TabDragging>());

        drag(&mut app, tab, window, PointerId::Mouse, Vec2::splat(4.0));
        assert_eq!(
            app.world().resource::<DragLifecycleLog>().0,
            [TabDragLifecycle {
                source: list,
                tab,
                pointer_id: PointerId::Mouse,
                phase: TabDragPhase::Started,
            }]
        );
        assert_eq!(
            app.world().entity(tab).get::<TabDragging>(),
            Some(&TabDragging {
                pointer_id: PointerId::Mouse
            })
        );
    }

    #[test]
    fn disabled_mode_and_locked_tabs_do_not_start_drag_bookkeeping() {
        let (mut app, window) = tab_app();
        let disabled_list = app
            .world_mut()
            .spawn((TabList::default(), ChildOf(window)))
            .id();
        let disabled_mode_tab = app.world_mut().spawn((Tab, ChildOf(disabled_list))).id();
        let reorder_list = app
            .world_mut()
            .spawn((
                TabList {
                    drag: TabDragMode::Reorder,
                    ..Default::default()
                },
                ChildOf(window),
            ))
            .id();
        let locked_tab = app
            .world_mut()
            .spawn((Tab, TabLocked, ChildOf(reorder_list)))
            .id();
        app.update();

        for (pointer_id, tab) in [
            (PointerId::Mouse, disabled_mode_tab),
            (PointerId::Touch(1), locked_tab),
        ] {
            start_drag(&mut app, tab, window, pointer_id);
            drag(&mut app, tab, window, pointer_id, Vec2::splat(20.0));
            assert!(!app.world().entity(tab).contains::<TabDragging>());
        }
        assert!(app.world().resource::<DragLifecycleLog>().0.is_empty());
        assert!(app.world().entity(locked_tab).contains::<Selectable>());
    }

    #[test]
    fn same_strip_drop_emits_post_removal_index_without_mutating_hierarchy() {
        let (mut app, window) = tab_app();
        let list = app
            .world_mut()
            .spawn((
                TabList {
                    drag: TabDragMode::Reorder,
                    ..Default::default()
                },
                ChildOf(window),
            ))
            .id();
        let first = app
            .world_mut()
            .spawn((
                Tab,
                ComputedNode::default(),
                UiGlobalTransform::from_xy(50.0, 20.0),
                ChildOf(list),
            ))
            .id();
        let second = app
            .world_mut()
            .spawn((
                Tab,
                ComputedNode::default(),
                UiGlobalTransform::from_xy(150.0, 20.0),
                ChildOf(list),
            ))
            .id();
        let third = app
            .world_mut()
            .spawn((
                Tab,
                ComputedNode::default(),
                UiGlobalTransform::from_xy(250.0, 20.0),
                ChildOf(list),
            ))
            .id();
        app.update();
        app.world_mut()
            .resource_mut::<HoverMap>()
            .entry(PointerId::Mouse)
            .or_default()
            .insert(list, HitData::new(window, 0.0, None, None));
        let original_order = [first, second, third];

        start_drag(&mut app, first, window, PointerId::Mouse);
        drag_at(
            &mut app,
            first,
            window,
            PointerId::Mouse,
            Vec2::new(350.0, 0.0),
            Vec2::new(400.0, 20.0),
        );
        assert_eq!(
            app.world().entity(list).get::<TabInsertionPreview>(),
            Some(&TabInsertionPreview {
                entries: vec![TabInsertionPoint {
                    pointer_id: PointerId::Mouse,
                    tab: first,
                    index: 2,
                }],
            })
        );
        assert_eq!(
            app.world().entity(list).get::<Children>().unwrap().as_ref(),
            original_order
        );

        end_drag_at(
            &mut app,
            first,
            window,
            PointerId::Mouse,
            Vec2::new(350.0, 0.0),
            Vec2::new(400.0, 20.0),
        );

        assert_eq!(
            app.world().resource::<TabMoveLog>().0,
            [TabMoved {
                from_strip: list,
                tab: first,
                to_strip: list,
                index: 2,
            }]
        );
        assert_eq!(
            app.world().entity(list).get::<Children>().unwrap().as_ref(),
            original_order
        );
        assert!(app
            .world()
            .entity(list)
            .get::<TabInsertionPreview>()
            .is_none());
    }

    #[test]
    fn external_drop_uses_destination_order_for_cross_strip_index() {
        let (mut app, window) = tab_app();
        let source = app
            .world_mut()
            .spawn((
                TabList {
                    drag: TabDragMode::External,
                    ..Default::default()
                },
                ChildOf(window),
            ))
            .id();
        let destination = app
            .world_mut()
            .spawn((
                TabList {
                    drag: TabDragMode::External,
                    ..Default::default()
                },
                ChildOf(window),
            ))
            .id();
        let dragged = app.world_mut().spawn((Tab, ChildOf(source))).id();
        for center in [50.0, 150.0, 250.0] {
            app.world_mut().spawn((
                Tab,
                ComputedNode::default(),
                UiGlobalTransform::from_xy(center, 20.0),
                ChildOf(destination),
            ));
        }
        app.update();
        app.world_mut()
            .resource_mut::<HoverMap>()
            .entry(PointerId::Mouse)
            .or_default()
            .insert(destination, HitData::new(window, 0.0, None, None));

        start_drag(&mut app, dragged, window, PointerId::Mouse);
        drag_at(
            &mut app,
            dragged,
            window,
            PointerId::Mouse,
            Vec2::new(100.0, 0.0),
            Vec2::new(100.0, 20.0),
        );
        end_drag_at(
            &mut app,
            dragged,
            window,
            PointerId::Mouse,
            Vec2::new(100.0, 0.0),
            Vec2::new(100.0, 20.0),
        );

        assert_eq!(
            app.world().resource::<TabMoveLog>().0,
            [TabMoved {
                from_strip: source,
                tab: dragged,
                to_strip: destination,
                index: 1,
            }]
        );
        assert_eq!(
            app.world()
                .entity(dragged)
                .get::<ChildOf>()
                .map(ChildOf::parent),
            Some(source),
            "the headless widget must not reparent the tab"
        );
    }

    #[test]
    fn external_drop_into_empty_strip_uses_zero_index() {
        let (mut app, window) = tab_app();
        let source = app
            .world_mut()
            .spawn((
                TabList {
                    drag: TabDragMode::External,
                    ..Default::default()
                },
                ChildOf(window),
            ))
            .id();
        let destination = app
            .world_mut()
            .spawn((
                TabList {
                    drag: TabDragMode::External,
                    ..Default::default()
                },
                ChildOf(window),
            ))
            .id();
        let dragged = app.world_mut().spawn((Tab, ChildOf(source))).id();
        app.update();
        app.world_mut()
            .resource_mut::<HoverMap>()
            .entry(PointerId::Mouse)
            .or_default()
            .insert(destination, HitData::new(window, 0.0, None, None));

        start_drag(&mut app, dragged, window, PointerId::Mouse);
        drag(
            &mut app,
            dragged,
            window,
            PointerId::Mouse,
            Vec2::splat(20.0),
        );
        end_drag_at(
            &mut app,
            dragged,
            window,
            PointerId::Mouse,
            Vec2::splat(20.0),
            Vec2::ZERO,
        );

        assert_eq!(
            app.world().resource::<TabMoveLog>().0,
            [TabMoved {
                from_strip: source,
                tab: dragged,
                to_strip: destination,
                index: 0,
            }]
        );
    }

    #[test]
    fn cancelling_one_pointer_preserves_another_pointer_preview() {
        let (mut app, window) = tab_app();
        let source = app
            .world_mut()
            .spawn((
                TabList {
                    drag: TabDragMode::External,
                    ..Default::default()
                },
                ChildOf(window),
            ))
            .id();
        let destination = app
            .world_mut()
            .spawn((
                TabList {
                    drag: TabDragMode::External,
                    ..Default::default()
                },
                ChildOf(window),
            ))
            .id();
        let mouse_tab = app.world_mut().spawn((Tab, ChildOf(source))).id();
        let touch_tab = app.world_mut().spawn((Tab, ChildOf(source))).id();
        app.update();
        for pointer_id in [PointerId::Mouse, PointerId::Touch(1)] {
            app.world_mut()
                .resource_mut::<HoverMap>()
                .entry(pointer_id)
                .or_default()
                .insert(destination, HitData::new(window, 0.0, None, None));
        }

        start_drag(&mut app, mouse_tab, window, PointerId::Mouse);
        drag(
            &mut app,
            mouse_tab,
            window,
            PointerId::Mouse,
            Vec2::splat(20.0),
        );
        start_drag(&mut app, touch_tab, window, PointerId::Touch(1));
        drag(
            &mut app,
            touch_tab,
            window,
            PointerId::Touch(1),
            Vec2::splat(20.0),
        );
        cancel_drag(&mut app, touch_tab, window, PointerId::Touch(1));

        assert_eq!(
            app.world()
                .entity(destination)
                .get::<TabInsertionPreview>()
                .and_then(|preview| preview.entries.first())
                .map(|preview| preview.pointer_id),
            Some(PointerId::Mouse)
        );
    }

    #[test]
    fn the_same_tab_cannot_be_dragged_by_two_pointers() {
        let (mut app, window) = tab_app();
        let source = app
            .world_mut()
            .spawn((
                TabList {
                    drag: TabDragMode::Reorder,
                    ..Default::default()
                },
                ChildOf(window),
            ))
            .id();
        let tab = app.world_mut().spawn((Tab, ChildOf(source))).id();
        app.update();

        start_drag(&mut app, tab, window, PointerId::Mouse);
        start_drag(&mut app, tab, window, PointerId::Touch(1));
        drag(&mut app, tab, window, PointerId::Mouse, Vec2::splat(20.0));
        drag(
            &mut app,
            tab,
            window,
            PointerId::Touch(1),
            Vec2::splat(20.0),
        );

        assert_eq!(app.world().resource::<DragLifecycleLog>().0.len(), 1);
        assert_eq!(
            app.world().entity(tab).get::<TabDragging>(),
            Some(&TabDragging {
                pointer_id: PointerId::Mouse,
            })
        );
    }

    #[test]
    fn reorder_mode_rejects_other_strips() {
        let (mut app, window) = tab_app();
        let source = app
            .world_mut()
            .spawn((
                TabList {
                    drag: TabDragMode::Reorder,
                    ..Default::default()
                },
                ChildOf(window),
            ))
            .id();
        let other = app
            .world_mut()
            .spawn((
                TabList {
                    drag: TabDragMode::External,
                    ..Default::default()
                },
                ChildOf(window),
            ))
            .id();
        let dragged = app.world_mut().spawn((Tab, ChildOf(source))).id();
        app.update();
        app.world_mut()
            .resource_mut::<HoverMap>()
            .entry(PointerId::Mouse)
            .or_default()
            .insert(other, HitData::new(window, 0.0, None, None));

        start_drag(&mut app, dragged, window, PointerId::Mouse);
        drag(
            &mut app,
            dragged,
            window,
            PointerId::Mouse,
            Vec2::splat(20.0),
        );
        end_drag_at(
            &mut app,
            dragged,
            window,
            PointerId::Mouse,
            Vec2::splat(20.0),
            Vec2::ZERO,
        );

        assert!(app.world().resource::<TabMoveLog>().0.is_empty());
        assert_eq!(
            app.world()
                .resource::<DragLifecycleLog>()
                .0
                .last()
                .map(|event| &event.phase),
            Some(&TabDragPhase::Completed { drop: None })
        );
        assert!(app
            .world()
            .entity(other)
            .get::<TabInsertionPreview>()
            .is_none());
    }

    #[test]
    fn pointer_cancellation_clears_accepted_drag_without_moving_tab() {
        let (mut app, window) = tab_app();
        let list = app
            .world_mut()
            .spawn((
                TabList {
                    drag: TabDragMode::Reorder,
                    ..Default::default()
                },
                ChildOf(window),
            ))
            .id();
        let tab = app
            .world_mut()
            .spawn((
                Tab,
                ComputedNode::default(),
                UiGlobalTransform::from_xy(50.0, 20.0),
                ChildOf(list),
            ))
            .id();
        app.update();
        app.world_mut()
            .resource_mut::<HoverMap>()
            .entry(PointerId::Mouse)
            .or_default()
            .insert(list, HitData::new(window, 0.0, None, None));

        start_drag(&mut app, tab, window, PointerId::Mouse);
        drag(&mut app, tab, window, PointerId::Mouse, Vec2::splat(20.0));
        cancel_drag(&mut app, tab, window, PointerId::Mouse);

        assert_eq!(
            app.world()
                .resource::<DragLifecycleLog>()
                .0
                .last()
                .map(|event| &event.phase),
            Some(&TabDragPhase::Cancelled)
        );
        assert!(app.world().resource::<TabMoveLog>().0.is_empty());
        assert!(!app.world().entity(tab).contains::<TabDragging>());
        assert!(app
            .world()
            .entity(list)
            .get::<TabInsertionPreview>()
            .is_none());
        assert_eq!(
            app.world()
                .entity(tab)
                .get::<ChildOf>()
                .map(ChildOf::parent),
            Some(list)
        );
    }

    #[test]
    fn escape_cancels_all_accepted_tab_drags() {
        let (mut app, window) = tab_app();
        let list = app
            .world_mut()
            .spawn((
                TabList {
                    drag: TabDragMode::Reorder,
                    ..Default::default()
                },
                ChildOf(window),
            ))
            .id();
        let first = app.world_mut().spawn((Tab, ChildOf(list))).id();
        let second = app.world_mut().spawn((Tab, ChildOf(list))).id();
        app.world_mut()
            .resource_mut::<InputFocus>()
            .set(first, FocusCause::Navigated);
        app.update();

        for (pointer_id, tab) in [(PointerId::Mouse, first), (PointerId::Touch(1), second)] {
            start_drag(&mut app, tab, window, pointer_id);
            drag(&mut app, tab, window, pointer_id, Vec2::splat(20.0));
        }
        press_key(&mut app, KeyCode::Escape, window);

        assert!(!app.world().entity(first).contains::<TabDragging>());
        assert!(!app.world().entity(second).contains::<TabDragging>());
        assert_eq!(
            app.world()
                .resource::<DragLifecycleLog>()
                .0
                .iter()
                .filter(|event| event.phase == TabDragPhase::Cancelled)
                .count(),
            2
        );
        assert!(app.world().resource::<TabMoveLog>().0.is_empty());
    }

    #[test]
    fn source_despawn_cancels_accepted_drag() {
        let (mut app, window) = tab_app();
        let list = app
            .world_mut()
            .spawn((
                TabList {
                    drag: TabDragMode::Reorder,
                    ..Default::default()
                },
                ChildOf(window),
            ))
            .id();
        let tab = app.world_mut().spawn((Tab, ChildOf(list))).id();
        app.update();

        start_drag(&mut app, tab, window, PointerId::Mouse);
        drag(&mut app, tab, window, PointerId::Mouse, Vec2::splat(20.0));
        app.world_mut().despawn(tab);
        app.update();

        assert_eq!(
            app.world()
                .resource::<DragLifecycleLog>()
                .0
                .last()
                .map(|event| &event.phase),
            Some(&TabDragPhase::Cancelled)
        );
        assert!(app.world().resource::<TabMoveLog>().0.is_empty());
    }

    #[test]
    fn completed_drag_suppresses_only_the_following_click() {
        let (mut app, window) = tab_app();
        let list = app
            .world_mut()
            .spawn((
                TabList {
                    drag: TabDragMode::Reorder,
                    ..Default::default()
                },
                ChildOf(window),
            ))
            .id();
        let tab = app
            .world_mut()
            .spawn((
                Tab,
                ComputedNode::default(),
                UiGlobalTransform::from_xy(50.0, 20.0),
                ChildOf(list),
            ))
            .id();
        app.update();
        app.world_mut()
            .resource_mut::<HoverMap>()
            .entry(PointerId::Mouse)
            .or_default()
            .insert(list, HitData::new(window, 0.0, None, None));

        start_drag(&mut app, tab, window, PointerId::Mouse);
        drag(&mut app, tab, window, PointerId::Mouse, Vec2::splat(20.0));
        end_drag_at_without_update(
            &mut app,
            tab,
            window,
            PointerId::Mouse,
            Vec2::splat(20.0),
            Vec2::ZERO,
        );

        click(&mut app, tab, window);
        assert!(app.world().resource::<SelectionRequests>().0.is_empty());

        click(&mut app, tab, window);
        assert_eq!(
            app.world().resource::<SelectionRequests>().0,
            [(list, Some(tab))]
        );
    }

    #[test]
    fn click_in_a_later_frame_after_drag_completion_requests_selection() {
        let (mut app, window) = tab_app();
        let list = app
            .world_mut()
            .spawn((
                TabList {
                    drag: TabDragMode::Reorder,
                    ..Default::default()
                },
                ChildOf(window),
            ))
            .id();
        let tab = app
            .world_mut()
            .spawn((
                Tab,
                ComputedNode::default(),
                UiGlobalTransform::from_xy(50.0, 20.0),
                ChildOf(list),
            ))
            .id();
        app.update();
        app.world_mut()
            .resource_mut::<HoverMap>()
            .entry(PointerId::Mouse)
            .or_default()
            .insert(list, HitData::new(window, 0.0, None, None));

        start_drag(&mut app, tab, window, PointerId::Mouse);
        drag(&mut app, tab, window, PointerId::Mouse, Vec2::splat(20.0));
        end_drag_at(
            &mut app,
            tab,
            window,
            PointerId::Mouse,
            Vec2::splat(20.0),
            Vec2::ZERO,
        );

        click(&mut app, tab, window);

        assert_eq!(
            app.world().resource::<SelectionRequests>().0,
            [(list, Some(tab))]
        );
    }
}
