use criterion::{criterion_group, Criterion};
use std::time::Duration;

use bevy_app::App;
use bevy_scene2::prelude::*;
use bevy_ui::prelude::*;

criterion_group!(benches, spawn);

fn ui() -> impl Scene {
    bsn! {
        Node {
            width: Val::Percent(100.0),
            height: Val::Percent(100.0),
            align_items: AlignItems::Center,
            justify_content: JustifyContent::Center,
        } [
            :button("Button")
        ]
    }
}

fn button(label: &'static str) -> impl Scene {
    bsn! {
        Button
        Node {
            width: Val::Px(150.0),
            height: Val::Px(65.0),
            border: UiRect::all(Val::Px(5.0)),
            justify_content: JustifyContent::Center,
            align_items: AlignItems::Center,
        }
        [(
            Text(label)
            TextShadow
        )]
    }
}

fn spawn(c: &mut Criterion) {
    let mut group = c.benchmark_group("spawn");
    group.warm_up_time(Duration::from_millis(500));
    group.measurement_time(Duration::from_secs(4));
    group.bench_function("ui_immediate", |b| {
        let mut app = App::new();
        app.add_plugins((
            bevy_asset::AssetPlugin::default(),
            bevy_scene2::ScenePlugin::default(),
        ));

        b.iter(move || {
            app.world_mut().spawn_scene_immediate(ui());
        });
    });
    group.finish();
}
