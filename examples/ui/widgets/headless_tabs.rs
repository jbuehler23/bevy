//! Demonstrates the behavior-only tab widgets in `bevy_ui_widgets`.

use bevy::{
    ecs::template::{EntityTemplate, OptionTemplate},
    input_focus::{
        tab_navigation::{TabGroup, TabNavigationPlugin},
        InputFocus, InputFocusVisible,
    },
    picking::{hover::Hovered, pointer::PointerId},
    prelude::*,
    ui::{InteractionDisabled, Selected},
    ui_widgets::{
        tablist_self_update, ControlOrientation, DragOverlayRoot, DragProxy, SelectedTab, Tab,
        TabActivation, TabDragLifecycle, TabDragMode, TabDragPhase, TabInsertionPreview, TabList,
        TabLocked, TabMoved, ValueChange,
    },
};

#[derive(Component, Default, Clone)]
struct ShowcaseTab;

#[derive(Component, Default, Clone)]
struct DraggableStrip;

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
enum ExternalDropOutcome {
    #[default]
    Idle,
    Moved,
    Dropped,
    Missed,
    Cancelled,
}

#[derive(Component, Default, Clone)]
struct ExternalDropTarget {
    active_tab: Option<Entity>,
    outcome: ExternalDropOutcome,
}

#[derive(Component, Default, Clone)]
struct ExternalDropBadge;

#[derive(Component, Default, Clone)]
struct ExternalDropStatus;

#[derive(Component, Default, Clone)]
struct ExternalDropTray;

#[derive(Component, Clone)]
struct ShowcaseInsertionIndicator {
    pointer_id: PointerId,
}

#[derive(Resource, Default)]
struct ControlledSelection(Option<Entity>);

fn main() {
    App::new()
        .add_plugins((DefaultPlugins, TabNavigationPlugin))
        .init_resource::<ControlledSelection>()
        .add_observer(observe_external_drop_lifecycle)
        .add_systems(Startup, showcase.spawn())
        .add_systems(
            Update,
            (
                update_tab_styles,
                update_external_drop_target_visuals,
                update_insertion_preview_styles,
                update_insertion_indicators,
            ),
        )
        .run();
}

fn showcase() -> impl SceneList {
    bsn! {
        Camera2d
        --
        Node {
            width: percent(100),
            height: percent(100),
            padding: UiRect::all(px(24)),
            display: Display::Flex,
            flex_direction: FlexDirection::Column,
            align_items: AlignItems::Start,
            row_gap: px(18),
            overflow: Overflow::scroll_y(),
        }
        BackgroundColor(Color::srgb(0.06, 0.07, 0.09))
        TabGroup
        DragOverlayRoot
        Children [
            @section_label("Horizontal automatic - self-updating")
            --
            #automatic
            @tab_strip(ControlOrientation::Horizontal)
            TabList {
                orientation: ControlOrientation::Horizontal,
                activation: TabActivation::Automatic,
            }
            @selected_tab(#automatic_general)
            on(tablist_self_update)
            Children [
                #automatic_general
                @tab_header("General")
                --
                @tab_header("Rendering")
                --
                @tab_header("Disabled")
                InteractionDisabled
            ]
            --
            @section_label("Horizontal manual - focus and selection are separate")
            --
            @tab_strip(ControlOrientation::Horizontal)
            TabList::default()
            @selected_tab(#manual_scene)
            on(tablist_self_update)
            Children [
                #manual_scene @tab_header("Scene")
                --
                @tab_header("Assets")
                --
                @tab_header("Inspector")
            ]
            --
            @section_label("Vertical manual")
            --
            @tab_strip(ControlOrientation::Vertical)
            TabList {
                orientation: ControlOrientation::Vertical,
                activation: TabActivation::Manual,
            }
            @selected_tab(#vertical_transform)
            on(tablist_self_update)
            Children [
                #vertical_transform @tab_header("Transform")
                --
                @tab_header("Visibility")
                --
                @tab_header("Metadata")
            ]
            --
            @section_label("Controlled - observer updates external state")
            --
            @tab_strip(ControlOrientation::Horizontal)
            TabList::default()
            @selected_tab(#controlled_a)
            on(controlled_selection)
            Children [
                #controlled_a @tab_header("External A")
                --
                @tab_header("External B")
            ]
            --
            @section_label("Reorder - drag past a tab center")
            --
            DraggableStrip
            @tab_strip(ControlOrientation::Horizontal)
            TabList {
                orientation: ControlOrientation::Horizontal,
                activation: TabActivation::Manual,
                drag: TabDragMode::Reorder,
            }
            on(tablist_self_update)
            on(apply_tab_move)
            on(spawn_drag_proxy)
            Children [
                @tab_header("Outline")
                --
                @tab_header("Locked")
                TabLocked
                --
                @tab_header("Timeline")
            ]
            --
            @section_label("External - drag tabs between compatible strips")
            --
            Node {
                display: Display::Flex,
                flex_direction: FlexDirection::Row,
                column_gap: px(24),
            }
            Children [
                DraggableStrip
                @tab_strip(ControlOrientation::Horizontal)
                TabList {
                    orientation: ControlOrientation::Horizontal,
                    activation: TabActivation::Manual,
                    drag: TabDragMode::External,
                }
                on(tablist_self_update)
                on(apply_tab_move)
                on(spawn_drag_proxy)
                Children [
                    @tab_header("Project")
                    --
                    @tab_header("Assets")
                ]
                --
                DraggableStrip
                @tab_strip(ControlOrientation::Horizontal)
                TabList {
                    orientation: ControlOrientation::Horizontal,
                    activation: TabActivation::Manual,
                    drag: TabDragMode::External,
                }
                on(tablist_self_update)
                on(apply_tab_move)
                on(spawn_drag_proxy)
                Children [
                    @tab_header("Inspector")
                    --
                    @tab_header("Console")
                ]
            ]
            --
            ExternalDropTarget::default()
            Hovered
            Node {
                width: px(480),
                min_height: px(104),
                padding: UiRect::all(px(12)),
                border: UiRect::all(px(2)),
                display: Display::Flex,
                flex_direction: FlexDirection::Column,
                align_items: AlignItems::Stretch,
                row_gap: px(8),
            }
            BackgroundColor(Color::srgb(0.09, 0.10, 0.13))
            BorderColor::all(Color::srgb(0.28, 0.30, 0.36))
            Children [
                Node {
                    width: percent(100),
                    display: Display::Flex,
                    flex_direction: FlexDirection::Row,
                    align_items: AlignItems::Center,
                    justify_content: JustifyContent::SpaceBetween,
                }
                Children [
                    Text("Detach tab")
                    TextFont {
                        font_size: FontSize::Px(16.0)
                    }
                    TextColor(Color::srgb(0.90, 0.92, 0.96))
                    --
                    ExternalDropBadge
                    Text("IDLE")
                    TextFont {
                        font_size: FontSize::Px(12.0)
                    }
                    TextColor(Color::srgb(0.58, 0.64, 0.72))
                ]
                --
                ExternalDropStatus
                Text("Drag an external tab here")
                TextFont {
                    font_size: FontSize::Px(13.0)
                }
                TextColor(Color::srgb(0.67, 0.71, 0.78))
                --
                ExternalDropTray
                Node {
                    min_height: px(38),
                    display: Display::Flex,
                    flex_direction: FlexDirection::Row,
                    align_items: AlignItems::Center,
                    column_gap: px(6),
                }
                Children [
                    Text("Detached tray:")
                    TextFont {
                        font_size: FontSize::Px(12.0)
                    }
                    TextColor(Color::srgb(0.48, 0.53, 0.61))
                ]
            ]
        ]
    }
}

fn tab_strip(orientation: ControlOrientation) -> impl Scene {
    let flex_direction = match orientation {
        ControlOrientation::Horizontal => FlexDirection::Row,
        ControlOrientation::Vertical => FlexDirection::Column,
    };
    bsn! {
        Node {
            display: Display::Flex,
            flex_direction,
            align_items: AlignItems::Stretch,
            border: UiRect::all(px(2)),
        }
        BackgroundColor(Color::srgb(0.10, 0.11, 0.14))
        BorderColor::all(Color::srgb(0.10, 0.11, 0.14))
    }
}

fn tab_header(label: &'static str) -> impl Scene {
    bsn! {
        ShowcaseTab
        Tab
        Hovered
        Node {
            min_width: px(112),
            min_height: px(36),
            padding: UiRect::axes(px(12), px(8)),
            border: UiRect::all(px(1)),
            align_items: AlignItems::Center,
            justify_content: JustifyContent::Center,
        }
        BackgroundColor(Color::srgb(0.13, 0.14, 0.18))
        BorderColor::all(Color::srgb(0.24, 0.25, 0.30))
        Children [
            Text(label)
            TextFont {
                font_size: FontSize::Px(16.0)
            }
            TextColor(Color::srgb(0.88, 0.89, 0.92))
        ]
    }
}

fn selected_tab(tab: EntityTemplate) -> impl Scene {
    let tab = OptionTemplate::Some(tab);
    bsn! {
        SelectedTab({tab})
    }
}

fn section_label(label: &'static str) -> impl Scene {
    bsn! {
        Text(label)
        TextFont {
            font_size: FontSize::Px(17.0)
        }
        TextColor(Color::srgb(0.72, 0.76, 0.84))
    }
}

fn controlled_selection(
    change: On<ValueChange<Option<Entity>>>,
    mut state: ResMut<ControlledSelection>,
    mut commands: Commands,
) {
    state.0 = change.value;
    commands.entity(change.source).insert(SelectedTab(state.0));
}

fn apply_tab_move(
    moved: On<TabMoved>,
    children: Query<&Children>,
    tabs: Query<(), With<Tab>>,
    mut commands: Commands,
) {
    let event = moved.event();
    let Ok(destination_children) = children.get(event.to_strip) else {
        commands.entity(event.to_strip).add_child(event.tab);
        return;
    };

    let remaining_children = destination_children
        .iter()
        .filter(|child| *child != event.tab)
        .collect::<Vec<_>>();
    let mut tab_index = 0;
    let mut insertion_child_index = remaining_children.len();
    for (child_index, child) in remaining_children.iter().enumerate() {
        if tabs.contains(*child) {
            if tab_index == event.index {
                insertion_child_index = child_index;
                break;
            }
            tab_index += 1;
        }
    }

    commands
        .entity(event.to_strip)
        .insert_child(insertion_child_index, event.tab);
}

fn spawn_drag_proxy(
    event: On<TabDragLifecycle>,
    tabs: Query<(&Children, &Node, &BackgroundColor, &BorderColor), With<Tab>>,
    captions: Query<(&Text, &TextFont, &TextColor)>,
    mut commands: Commands,
) {
    if event.phase != TabDragPhase::Started {
        return;
    }
    let Ok((children, tab_node, background, border)) = tabs.get(event.tab) else {
        return;
    };
    let Some((caption, font, text_color)) =
        children.iter().find_map(|child| captions.get(child).ok())
    else {
        return;
    };

    let ghost_border = BorderColor {
        top: border.top.with_alpha(0.82),
        right: border.right.with_alpha(0.82),
        bottom: border.bottom.with_alpha(0.82),
        left: border.left.with_alpha(0.82),
    };
    commands.spawn((
        tab_node.clone(),
        BackgroundColor(background.0.with_alpha(0.78)),
        ghost_border,
        GlobalZIndex(100),
        ChildOf(event.tab),
        DragProxy {
            pointer_id: event.pointer_id,
            offset: Vec2::new(12.0, 12.0),
        },
        children![(
            Text::new(caption.0.clone()),
            font.clone(),
            TextColor(text_color.0.with_alpha(0.82)),
        )],
    ));
}

fn observe_external_drop_lifecycle(
    event: On<TabDragLifecycle>,
    tablists: Query<&TabList>,
    mut targets: Query<(&Hovered, &mut ExternalDropTarget)>,
    trays: Query<Entity, With<ExternalDropTray>>,
    mut commands: Commands,
) {
    let Ok(source) = tablists.get(event.source) else {
        return;
    };
    if source.drag != TabDragMode::External {
        return;
    }

    for (hovered, mut target) in &mut targets {
        match &event.phase {
            TabDragPhase::Started => {
                target.active_tab = Some(event.tab);
                target.outcome = ExternalDropOutcome::Idle;
            }
            TabDragPhase::Completed { drop: Some(_) } => {
                target.active_tab = None;
                target.outcome = ExternalDropOutcome::Moved;
            }
            phase @ TabDragPhase::Completed { drop: None } => {
                let accepts = target.active_tab == Some(event.tab)
                    && accepts_external_drop(phase, hovered.get(), source.drag);
                target.active_tab = None;
                if accepts {
                    if let Some(tray) = trays.iter().next() {
                        commands.entity(tray).add_child(event.tab);
                        target.outcome = ExternalDropOutcome::Dropped;
                    } else {
                        target.outcome = ExternalDropOutcome::Missed;
                    }
                } else {
                    target.outcome = ExternalDropOutcome::Missed;
                }
            }
            TabDragPhase::Cancelled => {
                target.active_tab = None;
                target.outcome = ExternalDropOutcome::Cancelled;
            }
        }
    }
}

struct ExternalDropPresentation {
    badge: &'static str,
    status: &'static str,
    border: Color,
    background: Color,
    text: Color,
}

fn external_drop_presentation(
    active: bool,
    hovered: bool,
    outcome: ExternalDropOutcome,
) -> ExternalDropPresentation {
    if active && hovered {
        return ExternalDropPresentation {
            badge: "READY",
            status: "Release to detach the tab",
            border: Color::srgb(1.0, 0.68, 0.22),
            background: Color::srgb(0.17, 0.14, 0.08),
            text: Color::srgb(1.0, 0.76, 0.34),
        };
    }
    if active {
        return ExternalDropPresentation {
            badge: "DRAGGING",
            status: "Move over this target or press Escape",
            border: Color::srgb(0.38, 0.67, 0.92),
            background: Color::srgb(0.09, 0.13, 0.18),
            text: Color::srgb(0.50, 0.76, 1.0),
        };
    }

    match outcome {
        ExternalDropOutcome::Idle => ExternalDropPresentation {
            badge: "IDLE",
            status: "Drag an external tab here",
            border: Color::srgb(0.28, 0.30, 0.36),
            background: Color::srgb(0.09, 0.10, 0.13),
            text: Color::srgb(0.58, 0.64, 0.72),
        },
        ExternalDropOutcome::Moved => ExternalDropPresentation {
            badge: "MOVED",
            status: "The destination strip handled the drop",
            border: Color::srgb(0.38, 0.67, 0.92),
            background: Color::srgb(0.09, 0.13, 0.18),
            text: Color::srgb(0.50, 0.76, 1.0),
        },
        ExternalDropOutcome::Dropped => ExternalDropPresentation {
            badge: "DETACHED",
            status: "The target moved the tab into its tray",
            border: Color::srgb(0.38, 0.76, 0.48),
            background: Color::srgb(0.08, 0.16, 0.11),
            text: Color::srgb(0.48, 0.88, 0.58),
        },
        ExternalDropOutcome::Missed => ExternalDropPresentation {
            badge: "NO DROP",
            status: "Release over this target to detach",
            border: Color::srgb(0.76, 0.42, 0.36),
            background: Color::srgb(0.17, 0.09, 0.08),
            text: Color::srgb(0.94, 0.54, 0.46),
        },
        ExternalDropOutcome::Cancelled => ExternalDropPresentation {
            badge: "CANCELLED",
            status: "No hierarchy change was applied",
            border: Color::srgb(0.76, 0.42, 0.36),
            background: Color::srgb(0.17, 0.09, 0.08),
            text: Color::srgb(0.94, 0.54, 0.46),
        },
    }
}

fn accepts_external_drop(phase: &TabDragPhase, hovered: bool, source_mode: TabDragMode) -> bool {
    source_mode == TabDragMode::External
        && hovered
        && matches!(phase, TabDragPhase::Completed { drop: None })
}

fn update_external_drop_target_visuals(
    mut targets: Query<
        (
            &ExternalDropTarget,
            &Hovered,
            &mut BorderColor,
            &mut BackgroundColor,
        ),
        Without<ExternalDropBadge>,
    >,
    mut badges: Query<(&mut Text, &mut TextColor), With<ExternalDropBadge>>,
    mut statuses: Query<&mut Text, (With<ExternalDropStatus>, Without<ExternalDropBadge>)>,
) {
    let Some((target, hovered, mut border, mut background)) = targets.iter_mut().next() else {
        return;
    };
    let presentation =
        external_drop_presentation(target.active_tab.is_some(), hovered.get(), target.outcome);
    border.set_all(presentation.border);
    background.0 = presentation.background;
    for (mut badge, mut color) in &mut badges {
        badge.0 = presentation.badge.to_owned();
        color.0 = presentation.text;
    }
    for mut status in &mut statuses {
        status.0 = presentation.status.to_owned();
    }
}

fn update_insertion_preview_styles(
    mut strips: Query<
        (Has<TabInsertionPreview>, &mut BorderColor),
        (With<DraggableStrip>, With<TabList>),
    >,
) {
    for (previewing, mut border) in &mut strips {
        border.set_all(if previewing {
            Color::srgb(1.0, 0.68, 0.22)
        } else {
            Color::srgb(0.10, 0.11, 0.14)
        });
    }
}

fn update_insertion_indicators(
    strips: Query<
        (
            Entity,
            &TabList,
            &TabInsertionPreview,
            Option<&Children>,
            &ComputedNode,
            &UiGlobalTransform,
        ),
        With<DraggableStrip>,
    >,
    tabs: Query<(&ComputedNode, &UiGlobalTransform), With<Tab>>,
    indicators: Query<(Entity, &ChildOf, &ShowcaseInsertionIndicator)>,
    mut commands: Commands,
) {
    for (indicator, parent, marker) in &indicators {
        let has_preview = strips
            .get(parent.parent())
            .is_ok_and(|(_, _, preview, _, _, _)| {
                preview
                    .entries
                    .iter()
                    .any(|entry| entry.pointer_id == marker.pointer_id)
            });
        if !has_preview {
            commands.entity(indicator).try_despawn();
        }
    }

    for (strip, tablist, preview, children, strip_node, strip_transform) in &strips {
        for entry in &preview.entries {
            let existing = indicators.iter().find_map(|(indicator, parent, marker)| {
                (parent.parent() == strip && marker.pointer_id == entry.pointer_id)
                    .then_some(indicator)
            });
            let ordered_tabs = children
                .into_iter()
                .flat_map(RelationshipTarget::iter)
                .filter(|child| *child != entry.tab && tabs.contains(*child))
                .collect::<Vec<_>>();
            let strip_origin = strip_transform.translation
                - Vec2::new(strip_node.size().x, strip_node.size().y) * 0.5;
            let edge = insertion_edge(entry.index, tablist.orientation, &ordered_tabs, &tabs)
                .unwrap_or(match tablist.orientation {
                    ControlOrientation::Horizontal => strip_origin.x,
                    ControlOrientation::Vertical => strip_origin.y,
                });
            let (left, top, width, height) = match tablist.orientation {
                ControlOrientation::Horizontal => {
                    (edge - strip_origin.x - 1.5, 0.0, px(3), percent(100))
                }
                ControlOrientation::Vertical => {
                    (0.0, edge - strip_origin.y - 1.5, percent(100), px(3))
                }
            };
            let node = Node {
                position_type: PositionType::Absolute,
                left: px(left),
                top: px(top),
                width,
                height,
                ..default()
            };
            if let Some(indicator) = existing {
                commands.entity(indicator).insert(node);
            } else {
                commands.spawn((
                    ShowcaseInsertionIndicator {
                        pointer_id: entry.pointer_id,
                    },
                    node,
                    BackgroundColor(Color::srgb(1.0, 0.68, 0.22)),
                    Pickable::IGNORE,
                    ChildOf(strip),
                ));
            }
        }
    }
}

fn insertion_edge(
    index: usize,
    orientation: ControlOrientation,
    tabs: &[Entity],
    geometry: &Query<(&ComputedNode, &UiGlobalTransform), With<Tab>>,
) -> Option<f32> {
    let tab = tabs
        .get(index)
        .or_else(|| tabs.last())
        .and_then(|tab| geometry.get(*tab).ok())?;
    let (node, transform) = tab;
    let center = match orientation {
        ControlOrientation::Horizontal => transform.translation.x,
        ControlOrientation::Vertical => transform.translation.y,
    };
    let extent = match orientation {
        ControlOrientation::Horizontal => node.size().x,
        ControlOrientation::Vertical => node.size().y,
    };
    Some(if index < tabs.len() {
        center - extent * 0.5
    } else {
        center + extent * 0.5
    })
}

fn update_tab_styles(
    focus: Res<InputFocus>,
    focus_visible: Res<InputFocusVisible>,
    mut tabs: Query<
        (
            Entity,
            &Hovered,
            Has<Selected>,
            Has<InteractionDisabled>,
            &mut BackgroundColor,
            &mut BorderColor,
        ),
        With<ShowcaseTab>,
    >,
) {
    for (entity, hovered, selected, disabled, mut background, mut border) in &mut tabs {
        background.0 = match (disabled, selected, hovered.get()) {
            (true, _, _) => Color::srgb(0.10, 0.10, 0.11),
            (false, true, _) => Color::srgb(0.18, 0.34, 0.52),
            (false, false, true) => Color::srgb(0.20, 0.21, 0.26),
            _ => Color::srgb(0.13, 0.14, 0.18),
        };
        border.set_all(if focus_visible.0 && focus.get() == Some(entity) {
            Color::srgb(0.45, 0.78, 1.0)
        } else {
            Color::srgb(0.24, 0.25, 0.30)
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn external_drop_status_tracks_drag_and_hover_state() {
        assert_eq!(
            external_drop_presentation(false, false, ExternalDropOutcome::Idle).badge,
            "IDLE"
        );
        assert_eq!(
            external_drop_presentation(true, false, ExternalDropOutcome::Idle).badge,
            "DRAGGING"
        );
        assert_eq!(
            external_drop_presentation(true, true, ExternalDropOutcome::Idle).badge,
            "READY"
        );
        assert_eq!(
            external_drop_presentation(false, false, ExternalDropOutcome::Dropped).badge,
            "DETACHED"
        );
        assert_eq!(
            external_drop_presentation(false, false, ExternalDropOutcome::Cancelled).badge,
            "CANCELLED"
        );
    }

    #[test]
    fn external_target_accepts_only_unhandled_completion_while_hovered() {
        let unhandled = TabDragPhase::Completed { drop: None };
        let strip_move = TabDragPhase::Completed {
            drop: Some(bevy::ui_widgets::TabDrop {
                destination: Entity::PLACEHOLDER,
                index: 0,
            }),
        };

        assert!(accepts_external_drop(
            &unhandled,
            true,
            TabDragMode::External
        ));
        assert!(!accepts_external_drop(
            &unhandled,
            false,
            TabDragMode::External
        ));
        assert!(!accepts_external_drop(
            &unhandled,
            true,
            TabDragMode::Reorder
        ));
        assert!(!accepts_external_drop(
            &strip_move,
            true,
            TabDragMode::External
        ));
        assert!(!accepts_external_drop(
            &TabDragPhase::Cancelled,
            true,
            TabDragMode::External
        ));
    }

    #[test]
    fn move_observer_can_insert_into_empty_destination() {
        let mut app = App::new();
        app.add_observer(apply_tab_move);
        let source = app.world_mut().spawn(TabList::default()).id();
        let destination = app.world_mut().spawn(TabList::default()).id();
        let tab = app.world_mut().spawn((Tab, ChildOf(source))).id();
        app.update();

        app.world_mut().trigger(TabMoved {
            from_strip: source,
            tab,
            to_strip: destination,
            index: 0,
        });
        app.update();

        assert_eq!(
            app.world()
                .entity(tab)
                .get::<ChildOf>()
                .map(ChildOf::parent),
            Some(destination)
        );
    }
}
