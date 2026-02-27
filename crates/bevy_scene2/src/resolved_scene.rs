use crate::{PatchContext, ScenePatch};
use bevy_asset::Handle;
use bevy_ecs::{
    bundle::Bundle,
    entity::Entity,
    error::Result,
    relationship::Relationship,
    template::{ErasedTemplate, Template, TemplateContext},
    world::EntityWorldMut,
};
use bevy_utils::TypeIdMap;
use std::any::TypeId;

#[derive(Default)]
pub struct ResolvedScene {
    pub template_indices: TypeIdMap<usize>,
    pub templates: Vec<Box<dyn ErasedTemplate>>,
    pub inherited: Option<Handle<ScenePatch>>,
    // PERF: special casing children probably makes sense here
    pub related: TypeIdMap<ResolvedRelatedScenes>,
    pub entity_references: Vec<(usize, usize)>,
}

impl std::fmt::Debug for ResolvedScene {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ResolvedScene")
            .field("related", &self.related)
            .field("entity_references", &self.entity_references)
            .finish()
    }
}

impl ResolvedScene {
    pub fn apply(&self, context: &mut TemplateContext) -> Result {
        // if let Some(inherited) = &self.inherited {
        //     let mut scene_patches = context.resource_mut::<Assets<ScenePatch>>();
        //     if let Some(mut patch) = scene_patches.get_mut(inherited)
        //         && let Some(resolved_inherited) = &mut patch.resolved
        //     {
        //         resolved_inherited.apply(context);
        //     }
        // }

        if let Some((scope, index)) = self.entity_references.first().copied() {
            context
                .scoped_entities
                .set(context.entity_scopes, scope, index, context.entity.id());
        }
        for template in &self.templates {
            template.apply(context)?;
        }

        for related in self.related.values() {
            let target = context.entity.id();
            context.entity.world_scope(|world| -> Result {
                // TODO: I think we need to scan the scene and resolve entities ahead of time, in order to dedupe? Or is there a way to do that
                // at patch time?
                for scene in &related.scenes {
                    let mut entity = if let Some((scope, index)) =
                        scene.entity_references.first().copied()
                    {
                        let entity =
                            context
                                .scoped_entities
                                .get(world, context.entity_scopes, scope, index);
                        world.entity_mut(entity)
                    } else {
                        world.spawn_empty()
                    };
                    (related.insert)(&mut entity, target);
                    // PERF: this will result in an archetype move
                    scene.apply(&mut TemplateContext::new(
                        &mut entity,
                        context.scoped_entities,
                        context.entity_scopes,
                    ))?;
                }
                Ok(())
            })?;
        }

        Ok(())
    }

    pub fn get_or_insert_erased_template<'a>(
        &'a mut self,
        context: &mut PatchContext,
        type_id: TypeId,
        default: fn() -> Box<dyn ErasedTemplate>,
    ) -> &'a mut dyn ErasedTemplate {
        self.internal_get_or_insert_template_with(type_id, || {
            if let Some(inherited_scene) = context.inherited
                && let Some(resolved_inherited) = &inherited_scene.resolved
                && let Some(inherited_template) = resolved_inherited.get_erased_template(type_id)
            {
                inherited_template.clone_template()
            } else {
                default()
            }
        })
    }
    pub fn get_or_insert_template<
        'a,
        T: Template<Output: Bundle> + Default + Send + Sync + 'static,
    >(
        &'a mut self,
        context: &mut PatchContext,
    ) -> &'a mut T {
        self.get_or_insert_erased_template(context, TypeId::of::<T>(), || Box::new(T::default()))
            .downcast_mut()
            .unwrap()
    }

    pub fn get_erased_template(&self, type_id: TypeId) -> Option<&dyn ErasedTemplate> {
        let index = self.template_indices.get(&type_id)?;
        Some(&*self.templates[*index])
    }

    fn internal_get_or_insert_template_with(
        &mut self,
        type_id: TypeId,
        get_value: impl FnOnce() -> Box<dyn ErasedTemplate>,
    ) -> &mut dyn ErasedTemplate {
        let index = self.template_indices.entry(type_id).or_insert_with(|| {
            let index = self.templates.len();
            self.templates.push(get_value());
            index
        });
        self.templates
            .get_mut(*index)
            .map(|value| &mut **value)
            .unwrap()
    }

    pub fn push_template<T: Template<Output: Bundle> + Send + Sync + 'static>(
        &mut self,
        template: T,
    ) {
        self.push_template_erased(Box::new(template));
    }

    pub fn push_template_erased(&mut self, template: Box<dyn ErasedTemplate>) {
        self.templates.push(template);
    }
}

pub struct ResolvedRelatedScenes {
    pub scenes: Vec<ResolvedScene>,
    pub insert: fn(&mut EntityWorldMut, target: Entity),
}

impl std::fmt::Debug for ResolvedRelatedScenes {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ResolvedRelatedScenes")
            .field("scenes", &self.scenes)
            .finish()
    }
}

impl ResolvedRelatedScenes {
    pub fn new<R: Relationship>() -> Self {
        Self {
            scenes: Vec::new(),
            insert: |entity, target| {
                entity.insert(R::from(target));
            },
        }
    }
}
