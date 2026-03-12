use crate::{PatchContext, ScenePatch};
use bevy_asset::{AssetPath, Assets, Handle};
use bevy_ecs::{
    bundle::Bundle,
    entity::Entity,
    error::{BevyError, Result},
    relationship::Relationship,
    template::{
        EntityScopes, ErasedTemplate, ScopedEntities, ScopedEntityIndex, Template, TemplateContext,
    },
    world::{EntityWorldMut, World},
};
use bevy_utils::TypeIdMap;
use std::any::TypeId;
use thiserror::Error;

#[derive(Default)]
pub struct ResolvedScene {
    pub template_indices: TypeIdMap<usize>,
    pub templates: Vec<Box<dyn ErasedTemplate>>,
    pub inherited: Option<Handle<ScenePatch>>,
    // PERF: special casing Children might make sense here to avoid hashing
    pub related: TypeIdMap<ResolvedRelatedScenes>,
    /// A list of all [`ScopedEntityIndex`] values associated with this entity. There can be more than one if this scene uses
    /// "flattened" inheritance.
    pub entity_indices: Vec<ScopedEntityIndex>,
}

impl std::fmt::Debug for ResolvedScene {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ResolvedScene")
            .field("related", &self.related)
            .field("entity_indices", &self.entity_indices)
            .finish()
    }
}

impl ResolvedScene {
    pub fn spawn_or_apply<'w>(
        &self,
        world: &'w mut World,
        entity_scopes: &EntityScopes,
        scoped_entities: &mut ScopedEntities,
    ) -> Result<EntityWorldMut<'w>, ApplySceneError> {
        let mut entity = if let Some(scoped_entity_index) = self.entity_indices.first().copied() {
            let entity = scoped_entities.get(world, entity_scopes, scoped_entity_index);
            world.entity_mut(entity)
        } else {
            world.spawn_empty()
        };

        self.apply(&mut TemplateContext::new(
            &mut entity,
            scoped_entities,
            entity_scopes,
        ))?;

        Ok(entity)
    }

    pub fn apply(&self, context: &mut TemplateContext) -> Result<(), ApplySceneError> {
        if let Some(inherited) = &self.inherited {
            let scene_patches = context.resource::<Assets<ScenePatch>>();
            if let Some(patch) = scene_patches.get(inherited)
                && let Some(resolved_inherited) = &patch.resolved
            {
                let (inherited_scene, inherited_entity_scopes) = &*(resolved_inherited.clone());
                inherited_scene
                    .apply(&mut TemplateContext {
                        entity: context.entity,
                        // unflattened inherited scenes have their own entity scope
                        scoped_entities: &mut ScopedEntities::new(
                            inherited_entity_scopes.entity_len(),
                        ),
                        entity_scopes: inherited_entity_scopes,
                    })
                    .map_err(|e| ApplySceneError::InheritedSceneError {
                        inherited: inherited.path().map(|v| v.clone()),
                        error: Box::new(e),
                    })?;
            }
        }

        if let Some(scoped_entity_index) = self.entity_indices.first().copied() {
            context.scoped_entities.set(
                context.entity_scopes,
                scoped_entity_index,
                context.entity.id(),
            );
        }
        for template in &self.templates {
            template
                .apply(context)
                .map_err(|e| ApplySceneError::TemplateBuildError(e))?;
        }

        for (index, related) in self.related.values().enumerate() {
            let target = context.entity.id();
            context
                .entity
                .world_scope(|world| -> Result<(), ApplySceneError> {
                    for scene in &related.scenes {
                        let mut entity = if let Some(scoped_entity_index) =
                            scene.entity_indices.first().copied()
                        {
                            let entity = context.scoped_entities.get(
                                world,
                                context.entity_scopes,
                                scoped_entity_index,
                            );
                            world.entity_mut(entity)
                        } else {
                            world.spawn_empty()
                        };
                        (related.insert)(&mut entity, target);
                        // PERF: this will result in an archetype move
                        scene
                            .apply(&mut TemplateContext::new(
                                &mut entity,
                                context.scoped_entities,
                                context.entity_scopes,
                            ))
                            .map_err(|e| ApplySceneError::RelatedSceneError {
                                relationship: related.relationship_name,
                                index,
                                error: Box::new(e),
                            })?;
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
                && let Some(inherited_template) = resolved_inherited.0.get_erased_template(type_id)
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

#[derive(Error, Debug)]
pub enum ApplySceneError {
    #[error("Failed to build a Template in the current Scene: {0}")]
    TemplateBuildError(BevyError),
    #[error("Failed to apply the inherited Scene (asset path: \"{inherited:?}\"): {error}")]
    InheritedSceneError {
        inherited: Option<AssetPath<'static>>,
        error: Box<ApplySceneError>,
    },
    #[error("Failed to apply the related {relationship} Scene at index {index}: {error}")]
    RelatedSceneError {
        relationship: &'static str,
        index: usize,
        error: Box<ApplySceneError>,
    },
}

pub struct ResolvedRelatedScenes {
    pub scenes: Vec<ResolvedScene>,
    pub insert: fn(&mut EntityWorldMut, target: Entity),
    pub relationship_name: &'static str,
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
            relationship_name: std::any::type_name::<R>(),
        }
    }
}
