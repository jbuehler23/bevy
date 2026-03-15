use crate::{ResolveContext, ScenePatch};
use bevy_asset::{AssetPath, Assets, Handle, UntypedAssetId};
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

/// A final "spawnable" scene (usually produced by calling [`Scene::resolve`]). This consists of:
/// 1. A collection of [`Template`]s to apply to a spawned [`Entity`], which are stored as [`ErasedTemplate`]s.
/// 2. A collection of [`RelatedResolvedScenes`], which will be spawned as "related" entities (ex: [`Children`] entities).
/// 3. The inherited [`ScenePatch`] if it exists.
///
/// This uses "copy-on-write" behavior for inherited scenes. If a [`Template`] that the inherited scene has is requested, it will be
/// cloned (using [`Template::clone_template`]) and added to the current [`ResolvedScene`].
///
/// When applying this [`ResolvedScene`] to an [`Entity`], the inherited scene (including its children) is applied _first_. _Then_ this
/// [`ResolvedScene`] is applied.
///
/// [`Scene::resolve`]: crate::Scene::resolve
/// [`Children`]: bevy_ecs::hierarchy::Children
#[derive(Default)]
pub struct ResolvedScene {
    /// The collection of [`Template`]s to apply to a spawned [`Entity`]. This can have multiple copies of the same [`Template`].
    templates: Vec<Box<dyn ErasedTemplate>>,
    /// The collection of [`RelatedResolvedScenes`], which will be spawned as "related" entities (ex: [`Children`] entities).
    ///
    /// [`Children`]: bevy_ecs::hierarchy::Children
    // PERF: special casing Children might make sense here to avoid hashing
    pub related: TypeIdMap<RelatedResolvedScenes>,
    /// The inherited [`ScenePatch`] to apply _first_ before applying this [`ResolvedScene`].
    inherited: Option<Handle<ScenePatch>>,
    /// A [`TypeId`] to `templates` index mapping. If a [`Template`] is intended to be shared / patched across scenes, it should have
    template_indices: TypeIdMap<usize>,
    /// A list of all [`ScopedEntityIndex`] values associated with this entity. There can be more than one if this scene uses
    /// "flattened" inheritance.
    pub entity_indices: Vec<ScopedEntityIndex>,
}

impl std::fmt::Debug for ResolvedScene {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ResolvedScene")
            .field("inherited", &self.inherited)
            .field("template_types", &self.template_indices.keys())
            .field("related", &self.related)
            .field("entity_indices", &self.entity_indices)
            .finish()
    }
}

impl ResolvedScene {
    /// This will spawn a new [`Entity`], if it is not already present in [`ScopedEntities`], and call [`ResolvedScene::apply`] on it.
    /// If this entity _is_ already present in [`ScopedEntities`] (ex: something else already referenced this entity), then it will _just_
    /// call [`ResolvedScene::apply`].
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

    /// Applies this scene to the given [`TemplateContext`] (which holds an already-spawned [`EntityWorldMut`]).
    ///
    /// This will apply all of the [`Template`]s in this [`ResolvedScene`] to the entity in the [`TemplateContext`]. It will also
    /// spawn all of this [`ResolvedScene`]'s related entities.
    ///
    /// If this [`ResolvedScene`] inherits from another scene, that scene will be applied _first_.
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
                    .map_err(|e| ApplySceneError::InheritedSceneApplyError {
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

    pub fn get_or_insert_template<
        'a,
        T: Template<Output: Bundle> + Default + Send + Sync + 'static,
    >(
        &'a mut self,
        context: &mut ResolveContext,
    ) -> &'a mut T {
        self.get_or_insert_erased_template(context, TypeId::of::<T>(), || Box::new(T::default()))
            .as_any_mut()
            .downcast_mut()
            .unwrap()
    }

    /// This will get the [`ErasedTemplate`] for the given [`TypeId`], if it already exists in this [`ResolvedScene`]. If it doesn't exist,
    /// it will use the `default` function to create a new [`ErasedTemplate`]. _For correctness, the [`TypeId`] of the [`Template`] returned
    /// by `default` should match the passed in `type_id`_.
    ///
    /// This uses "copy-on-write" behavior for inherited scenes. If a [`Template`] that the inherited scene has is requested, it will be
    /// cloned (using [`Template::clone_template`]), added to the current [`ResolvedScene`], and returned.
    ///
    /// This will ignore [`Template`]s added to this scene using [`ResolvedScene::push_template`], as these are not registered as the "canonical"
    /// [`Template`] for a given [`TypeId`].
    pub fn get_or_insert_erased_template<'a, F>(
        &'a mut self,
        context: &mut ResolveContext,
        type_id: TypeId,
        default: F,
    ) -> &'a mut dyn ErasedTemplate
    where
        F: Fn() -> Box<dyn ErasedTemplate>,
    {
        self.internal_get_or_insert_template_with(type_id, || {
            if let Some(inherited_scene) = context.inherited
                && let Some(resolved_inherited) = &inherited_scene.resolved
                && let Some(inherited_template) =
                    resolved_inherited.0.get_direct_erased_template(type_id)
            {
                inherited_template.clone_template()
            } else {
                default()
            }
        })
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

    /// Returns the [`ErasedTemplate`] for the given `type_id`, if it exists in this [`ResolvedScene`]. This ignores scene inheritance.
    pub fn get_direct_erased_template(&self, type_id: TypeId) -> Option<&dyn ErasedTemplate> {
        let index = self.template_indices.get(&type_id)?;
        Some(&*self.templates[*index])
    }

    /// Adds the `template` to the "back" of the [`ResolvedScene`] (it will applied later than earlier [`Template`]s).
    pub fn push_template<T: Template<Output: Bundle> + Send + Sync + 'static>(
        &mut self,
        template: T,
    ) {
        self.push_template_erased(Box::new(template));
    }

    /// Adds the `template` to the "back" of the [`ResolvedScene`] (it will applied later than earlier [`Template`]s).
    pub fn push_template_erased(&mut self, template: Box<dyn ErasedTemplate>) {
        self.templates.push(template);
    }

    /// This will return the existing [`RelatedResolvedScenes`], if it exists. If not, a new empty [`RelatedResolvedScenes`] will be inserted and returned.
    ///
    /// This is used to add new related scenes and read existing related scenes.
    pub fn get_or_insert_related_resolved_scenes<R: Relationship>(
        &mut self,
    ) -> &mut RelatedResolvedScenes {
        self.related
            .entry(TypeId::of::<R>())
            .or_insert_with(RelatedResolvedScenes::new::<R>)
    }

    /// Configures this [`ResolvedScene`] to inherit from the given [`ScenePatch`].
    ///
    /// If this [`ResolvedScene`] already inherits from a scene, it will return [`InheritSceneError::MultipleInheritance`].
    /// If this [`ResolvedScene`] already has [`Template`]s or related scenes, it will return [`InheritSceneError::LateInheritance`].
    pub fn inherit(&mut self, handle: Handle<ScenePatch>) -> Result<(), InheritSceneError> {
        if let Some(inherited) = &self.inherited {
            return Err(InheritSceneError::MultipleInheritance {
                id: inherited.id().untyped(),
                path: inherited.path().map(|p| p.clone()),
            });
        }
        if !(self.templates.is_empty() && self.related.is_empty()) {
            return Err(InheritSceneError::LateInheritance {
                id: handle.id().untyped(),
                path: handle.path().map(|p| p.clone()),
            });
        }
        self.inherited = Some(handle);
        Ok(())
    }
}

/// The error returned by [`ResolvedScene::inherit`].
#[derive(Error, Debug)]
pub enum InheritSceneError {
    /// Caused when attempting to inherit from a second scene.
    #[error("Attempted to inherit from a second scene (id {id:?}, path: {path:?}), which is not allowed.")]
    MultipleInheritance {
        id: UntypedAssetId,
        path: Option<AssetPath<'static>>,
    },
    /// Caused when attempting to inherit when a [`ResolvedScene`] already has [`Template`]s or related scenes.
    #[error("Attempted to inherit from (id {id:?}, path: {path:?}), but the resolved scene already has templates. For correctness, inheritance should always come first.")]
    LateInheritance {
        id: UntypedAssetId,
        path: Option<AssetPath<'static>>,
    },
}

/// An error produced when calling [`ResolvedScene::apply`].
#[derive(Error, Debug)]
pub enum ApplySceneError {
    /// Caused when a [`Template`] fails to build
    #[error("Failed to build a Template in the current Scene: {0}")]
    TemplateBuildError(BevyError),
    /// Caused when the inherited [`ResolvedScene`] fails to [`ResolvedScene::apply`].
    #[error("Failed to apply the inherited Scene (asset path: \"{inherited:?}\"): {error}")]
    InheritedSceneApplyError {
        inherited: Option<AssetPath<'static>>,
        error: Box<ApplySceneError>,
    },
    /// Caused when a related [`ResolvedScene`] fails to [`ResolvedScene::apply`].
    #[error("Failed to apply the related {relationship} Scene at index {index}: {error}")]
    RelatedSceneError {
        relationship: &'static str,
        index: usize,
        error: Box<ApplySceneError>,
    },
}

/// A collection of [`ResolvedScene`]s that are related to a given [`ResolvedScene`] by a [`Relationship`].
/// Each [`ResolvedScene`] added here will be spawned as a new [`Entity`] when the "parent" [`ResolvedScene`] is spawned.
pub struct RelatedResolvedScenes {
    pub scenes: Vec<ResolvedScene>,
    pub insert: fn(&mut EntityWorldMut, target: Entity),
    pub relationship_name: &'static str,
}

impl std::fmt::Debug for RelatedResolvedScenes {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ResolvedRelatedScenes")
            .field("scenes", &self.scenes)
            .finish()
    }
}

impl RelatedResolvedScenes {
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
