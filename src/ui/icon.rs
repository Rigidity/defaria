//! Item icons: one image per item, drawn on the CPU (see [`raster`]) from
//! the block texture's tiles, and an [`ItemIcon`] component that shows one
//! in an [`ImageNode`].
//!
//! The images are drawn again whenever the tiles are rebuilt, so icons hot
//! reload with the textures. Each item keeps the same image handle the whole
//! time, so nodes showing it never need touching.

pub mod raster;

use bevy::{
    asset::RenderAssetUsages,
    image::{ImageSampler, ImageSamplerDescriptor},
    prelude::*,
    render::render_resource::{Extent3d, TextureDimension, TextureFormat},
};

use crate::{
    content::{Content, ItemState},
    voxel_material::BlockTiles,
};

pub struct IconPlugin;

impl Plugin for IconPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<ItemIcons>()
            .init_resource::<BlockTiles>()
            .add_systems(Update, draw_icons)
            .add_systems(PostUpdate, show_icons.in_set(super::UiWidgetSystems));
    }
}

/// Every item's icon image, by [`ItemState`]. Empty images until the block
/// texture's tiles have been built.
#[derive(Resource, Debug, Clone)]
pub struct ItemIcons(pub Vec<Handle<Image>>);

impl ItemIcons {
    pub fn get(&self, item: ItemState) -> Option<&Handle<Image>> {
        self.0.get(item.0 as usize)
    }
}

impl FromWorld for ItemIcons {
    fn from_world(world: &mut World) -> Self {
        let count = world.resource::<Content>().item_states.len();
        let images = world.resource::<Assets<Image>>();
        Self((0..count).map(|_| images.reserve_handle()).collect())
    }
}

/// Shows an item's icon in the entity's [`ImageNode`], or nothing for
/// `None`.
#[derive(Component, Reflect, Debug, Clone, Copy, Default, PartialEq, Eq)]
#[reflect(Component)]
#[require(ImageNode)]
pub struct ItemIcon(pub Option<ItemState>);

/// Draws every item's icon into its image, whenever the tiles change.
fn draw_icons(
    tiles: Res<BlockTiles>,
    content: Res<Content>,
    icons: Res<ItemIcons>,
    mut images: ResMut<Assets<Image>>,
) {
    if !tiles.is_changed() || tiles.0.is_empty() {
        return;
    }
    for (item, handle) in content.item_states.iter().zip(&icons.0) {
        let pixels = raster::item_icon(&content, item, &tiles.0);
        if let Err(error) = images.insert(handle, icon_image(pixels)) {
            error!("can't store the icon of {}: {error}", item.name);
        }
    }
}

fn icon_image(pixels: raster::IconPixels) -> Image {
    let mut image = Image::new(
        Extent3d {
            width: pixels.size,
            height: pixels.size,
            depth_or_array_layers: 1,
        },
        TextureDimension::D2,
        pixels.data,
        TextureFormat::Rgba8UnormSrgb,
        RenderAssetUsages::RENDER_WORLD,
    );
    image.sampler = if pixels.flat {
        ImageSampler::nearest()
    } else {
        ImageSampler::Descriptor(ImageSamplerDescriptor::linear())
    };
    image
}

/// Points each changed [`ItemIcon`]'s image node at its icon.
pub fn show_icons(
    icons: Res<ItemIcons>,
    mut nodes: Query<(&ItemIcon, &mut ImageNode), Changed<ItemIcon>>,
) {
    for (icon, mut node) in &mut nodes {
        match icon.0.and_then(|item| icons.get(item)) {
            Some(handle) => {
                node.image = handle.clone();
                node.color = Color::WHITE;
            }
            // An image node with no image draws a white box; hide it.
            None => {
                node.image = Handle::default();
                node.color = Color::NONE;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::content::base;

    fn app() -> App {
        let mut app = App::new();
        app.add_plugins((MinimalPlugins, AssetPlugin::default()))
            .init_asset::<Image>()
            .insert_resource(base().clone())
            .add_plugins(IconPlugin);
        app
    }

    #[test]
    fn icons_are_drawn_once_the_tiles_exist_and_shown_by_item_icon() {
        let mut app = app();
        let stone = base().default_item("defaria:stone").unwrap();
        let shown = app.world_mut().spawn(ItemIcon(Some(stone))).id();
        let empty = app.world_mut().spawn(ItemIcon(None)).id();
        app.update();

        let handle = app
            .world()
            .resource::<ItemIcons>()
            .get(stone)
            .unwrap()
            .clone();
        assert!(
            app.world()
                .resource::<Assets<Image>>()
                .get(&handle)
                .is_none()
        );
        let node = app.world().get::<ImageNode>(shown).unwrap();
        assert_eq!(node.image, handle);
        assert_eq!(
            app.world().get::<ImageNode>(empty).unwrap().color,
            Color::NONE
        );

        // The tiles arrive: every icon is drawn.
        let tile = [200u8, 100, 50, 255].repeat(256);
        app.insert_resource(BlockTiles(vec![tile; base().textures.len()]));
        app.update();
        let images = app.world().resource::<Assets<Image>>();
        for handle in &app.world().resource::<ItemIcons>().0 {
            assert!(images.get(handle).is_some());
        }
        let size = images.get(&handle).unwrap().size();
        assert_eq!(size, UVec2::splat(raster::BLOCK_ICON_SIZE));
    }
}
