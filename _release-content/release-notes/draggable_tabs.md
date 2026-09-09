---
title: Draggable and reorderable tabs
authors: ["@jbuehler23"]
pull_requests: []
---

Headless tabs in `bevy_ui_widgets` can now be dragged. `TabList::drag` selects the policy: `TabDragMode::Disabled` (the default), `Reorder` for moving tabs within one list, or `External` for moving tabs between lists that also opt into `External`. Add `TabLocked` to a tab to keep it focusable and activatable but not draggable.

A drag is accepted once the pointer moves past a small threshold. The dragged tab then carries `TabDragging`, the source list receives `TabDragLifecycle` events for the start, completion, and cancellation of the gesture, and the list under the pointer carries a `TabInsertionPreview` with the proposed insertion index for each active pointer. Escape, pointer cancellation, or despawning the tab cancels the drag.

As with selection, the widget never changes the hierarchy itself. Releasing over a compatible list emits `TabMoved` on the source list with the destination and an insertion index measured after removing the dragged tab, and the app applies it:

```rust
fn apply_tab_move(moved: On<TabMoved>, mut commands: Commands) {
    commands
        .entity(moved.to_strip)
        .insert_child(moved.index, moved.tab);
}
```

The `headless_tabs` example shows reordering within a strip, moving tabs between two strips, a drop target outside any strip, and a drag proxy that follows the pointer.
