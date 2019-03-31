use super::{
    Border, BorderRadius, BorderSide, BorderStyle, BoxShadow, Color, ComputedLayout, Image,
    RenderService, Text,
};
use crate::generated::Vector2f;
use crate::scene::{SurfaceId, SurfaceData};
use crate::temp;
use crate::text::{LaidGlyph, PangoService, TextShaper};
use gleam::gl::Gl;
use image;
use image::GenericImageView;
use std::fs::File;
use std::io::prelude::*;
use std::rc::Rc;
use std::sync::mpsc::{channel, Receiver};
use webrender::api::{
    AddImage, AlphaType, BorderDetails, BorderDisplayItem, BorderRadius as WRBorderRadius,
    BorderSide as WRBorderSide, BorderStyle as WRBorderStyle, BoxShadowClipMode,
    BoxShadowDisplayItem, ColorF, ColorU, DeviceIntSize, DisplayListBuilder, DocumentId, Epoch,
    FontInstanceKey, GlyphInstance, ImageData, ImageDescriptor,
    ImageDisplayItem, ImageFormat, ImageRendering, LayoutPoint, LayoutPrimitiveInfo, LayoutRect,
    LayoutSize, LayoutVector2D, NormalBorder, PipelineId, RectangleDisplayItem, RenderApi,
    ResourceUpdate, SpaceAndClipInfo, SpecificDisplayItem, TextDisplayItem, Transaction,
    HitTestFlags, WorldPoint, ComplexClipRegion, ClipMode,
};
use webrender::euclid::{TypedSideOffsets2D, TypedSize2D, TypedVector2D};
use webrender::{Renderer, RendererOptions};

pub struct WebrenderRenderService {
    text_shaper: PangoService,

    renderer: Renderer,
    render_api: RenderApi,
    document_id: DocumentId,
    rx: Receiver<()>,

    pub layout_size: LayoutSize,
    fb_size: DeviceIntSize,
    // so that we can reuse already uploaded images
    // this can be (periodically) cleaned up by simply going through all keys and
    // looking what has (not) been used in the last render (and can be evicted)
    // _uploaded_images: BTreeMap<String, ImageKey>
}

impl WebrenderRenderService {
    pub fn new(gl: Rc<Gl>) -> Self {
        let fb_size = DeviceIntSize::new(1024, 768);
        let layout_size = LayoutSize::new(fb_size.width as f32, fb_size.height as f32);

        let (renderer, mut render_api, rx) = Self::init_webrender(gl);
        let document_id = render_api.add_document(fb_size, 0);

        Self::load_fonts(&mut render_api, document_id, &rx);

        WebrenderRenderService {
            // find out how to share one ref with YogaService
            text_shaper: PangoService::new(),

            renderer,
            render_api,
            document_id,
            rx,

            layout_size,
            fb_size,
        }
    }

    // not complete (border-radius) but it might be fine for some time
    pub fn hit_test(&self, x: f32, y: f32) -> Option<SurfaceId> {
        let res = self.render_api.hit_test(self.document_id, Some(PIPELINE_ID), WorldPoint::new(x, y), HitTestFlags::empty());

        res.items.get(0).map(|item| item.tag.1)
    }

    pub fn resize() {}

    fn init_webrender(gl: Rc<Gl>) -> (Renderer, RenderApi, Receiver<()>) {
        // so that we can block until the frame is actually rendered
        let (tx, rx) = channel();

        let (renderer, sender) = Renderer::new(
            gl,
            Box::new(temp::Notifier(tx)),
            RendererOptions {
                device_pixel_ratio: 1.0,
                ..RendererOptions::default()
            },
            None,
        )
        .expect("couldn't init webrender");
        let render_api = sender.create_api();

        (renderer, render_api, rx)
    }

    fn load_fonts(render_api: &mut RenderApi, document_id: DocumentId, rx: &Receiver<()>) {
        let property = font_loader::system_fonts::FontPropertyBuilder::new()
            .family("Arial")
            .build();
        let (font, font_index) = font_loader::system_fonts::get(&property).unwrap();
        let font_key = render_api.generate_font_key();

        let mut tx = Transaction::new();
        tx.set_root_pipeline(PipelineId::dummy());
        tx.add_raw_font(font_key, font, font_index as u32);

        // TODO: support any size
        for font_size in [10, 12, 14, 16, 20, 24, 34, 40, 48].iter() {
            tx.add_font_instance(
                FontInstanceKey(font_key.0, *font_size),
                font_key,
                app_units::Au::from_px(*font_size as i32),
                None,
                None,
                Vec::new(),
            );
        }

        tx.generate_frame();
        render_api.send_transaction(document_id, tx);
        rx.recv().ok();
    }

    fn send_frame(&mut self, builder: DisplayListBuilder) {
        let mut tx = Transaction::new();

        // according to https://github.com/servo/webrender/wiki/Path-to-the-Screen
        tx.set_root_pipeline(PIPELINE_ID);
        tx.set_display_list(Epoch(0), None, self.layout_size, builder.finalize(), true);
        tx.generate_frame();

        self.render_api.send_transaction(self.document_id, tx);

        self.wait_for_frame();
    }

    // this needs rework (rendering should be in its own thread anyway) but it's good enough for now
    fn wait_for_frame(&mut self) {
        self.rx.recv().ok();

        self.renderer.update();
        self.renderer.render(self.fb_size).ok();
    }
}

impl RenderService for WebrenderRenderService {
    fn render(&mut self, surface: &SurfaceData, computed_layouts: Vec<ComputedLayout>) {
        debug!("render\n{:#?}", surface);

        let content_size = LayoutSize::new(computed_layouts[0].2, computed_layouts[0].3);
        let pipeline_id = PIPELINE_ID;

        let builder = {
            let mut context = RenderContext {
                computed_layouts,
                render_api: &mut self.render_api,
                text_shaper: &self.text_shaper,

                builder: DisplayListBuilder::with_capacity(
                    pipeline_id,
                    content_size.clone(),
                    BUILDER_CAPACITY,
                ),
                border_radius: WRBorderRadius::zero(),
                layout: LayoutPrimitiveInfo::new(content_size.into()),
                space_and_clip: SpaceAndClipInfo::root_scroll(PIPELINE_ID),
            };

            context.render_surface(surface);

            context.builder
        };

        self.send_frame(builder)
    }
}

struct RenderContext<'a> {
    computed_layouts: Vec<ComputedLayout>,
    render_api: &'a mut RenderApi,
    text_shaper: &'a dyn TextShaper,

    builder: DisplayListBuilder,
    border_radius: WRBorderRadius,
    layout: LayoutPrimitiveInfo,
    space_and_clip: SpaceAndClipInfo,
}

impl<'a> RenderContext<'a> {
    // TODO: scroll
    fn render_surface(&mut self, surface: &SurfaceData) {
        let parent_layout = self.layout;
        let parent_space_and_clip = self.space_and_clip;

        let (x, y, width, height) = self.computed_layouts[surface.id() as usize];

        self.layout = LayoutPrimitiveInfo::new(
            LayoutRect::new(LayoutPoint::new(x, y), LayoutSize::new(width, height))
                .translate(&parent_layout.rect.origin.to_vector()),
        );

        // TODO: surface flag
        // components/surfaces should be explicit about if they want to receive pointer events or not
        // ommitting tag works a bit like bubbling (closest parent is matched)
        //
        // interactive components should set this even if they don't have a callback yet
        // (button should always match, not be click-through)
        self.layout.tag = Some((0, surface.id()));

        debug!("surface {} {:?}", surface.id(), self.layout.rect);

        // shared, not directly rendered
        if let Some(border_radius) = surface.border_radius() {
            self.border_radius = border_radius.clone().into();

            let clip_region = ComplexClipRegion::new(self.layout.clip_rect.clone(), self.border_radius, ClipMode::Clip);
            let clip_id = self.builder.define_clip(&self.space_and_clip, self.layout.clip_rect, vec![clip_region], None);

            self.space_and_clip = self.space_and_clip.clone();
            self.space_and_clip.clip_id = clip_id;
        } else {
            self.border_radius = WRBorderRadius::zero();
        }

        if let Some(box_shadow) = surface.box_shadow() {
            let Vector2f(x, y) = box_shadow.offset;
            let size = box_shadow.spread + box_shadow.blur;
            let layout = LayoutPrimitiveInfo::with_clip_rect(
                self.layout.rect,
                self.layout
                    .rect
                    .translate(&TypedVector2D::new(x, y))
                    .inflate(size, size),
            );
            self.builder.push_item(
                &self.box_shadow(box_shadow.clone()),
                &layout,
                &self.space_and_clip,
            );
        }

        if let Some(color) = surface.background_color() {
            self.push(self.background_color(color.clone()));
        }

        if let Some(image) = surface.image() {
            self.push(self.image(image.clone()));
        }

        // TODO: selections (should be below text)

        if let Some(text) = surface.text() {
            let (item, glyphs) = self.text(text.clone());

            self.push(item);
            self.builder.push_iter(glyphs);
        }

        for child_surface in surface.children() {
            self.render_surface(&child_surface);
        }

        if let Some(border) = surface.border() {
            self.push(self.border(border.clone()));
        }

        // restore layout
        self.layout = parent_layout;
        self.space_and_clip = parent_space_and_clip;
    }

    fn box_shadow(&self, box_shadow: BoxShadow) -> SpecificDisplayItem {
        SpecificDisplayItem::BoxShadow(BoxShadowDisplayItem {
            color: box_shadow.color.clone().into(),

            box_bounds: self.layout.rect,
            offset: box_shadow.offset.into(),
            blur_radius: box_shadow.blur,
            spread_radius: box_shadow.spread,
            border_radius: self.border_radius.clone().into(),

            // TODO: Inset/Outset (outset needs bigger clip-rect)
            clip_mode: BoxShadowClipMode::Outset,
        })
    }

    fn background_color(&self, color: Color) -> SpecificDisplayItem {
        SpecificDisplayItem::Rectangle(RectangleDisplayItem {
            color: color.into(),
        })
    }

    // TODO: refactor, cache, free + hook to make loading possible from node.js (http)
    fn image(&self, image: Image) -> SpecificDisplayItem {
        let image_key = {
            let mut f = File::open(image.url).expect("couldn't open file");
            let mut buffer = Vec::new();
            f.read_to_end(&mut buffer).expect("couldn't read");

            let image = image::load_from_memory(&buffer).expect("couldn't load image");
            let descriptor = ImageDescriptor::new(
                image.width() as i32,
                image.height() as i32,
                ImageFormat::RGBA8,
                true,
                false,
            );

            let key = self.render_api.generate_image_key();

            self.render_api
                .update_resources(vec![ResourceUpdate::AddImage(AddImage {
                    key,
                    descriptor,
                    data: ImageData::new(
                        image
                            .as_rgba8()
                            .expect("couldn't convert to rgba8")
                            .to_vec(),
                    ),
                    tiling: None,
                })]);

            key
        };

        SpecificDisplayItem::Image(ImageDisplayItem {
            image_key,
            stretch_size: self.layout.rect.size.into(),
            tile_spacing: TypedSize2D::zero(),
            image_rendering: ImageRendering::Auto,
            alpha_type: AlphaType::PremultipliedAlpha,
            color: ColorF::WHITE,
        })
    }

    // TODO: cache, free, refactor, etc.
    // (this is rather PoC)
    // TODO: clip should be enough big to contain `y` and similar characters
    fn text(&self, text: Text) -> (SpecificDisplayItem, Vec<GlyphInstance>) {
        let [width, height] = self.layout.rect.size.to_array();
        let [text_x, text_y] = self.layout.rect.origin.to_array();

        // y is from the bottom, I think it should be y + ((text.line_height + text.font_size) / 2)
        // but this works better (no idea why)
        let text_y = text_y + (text.line_height / 2.) + (text.font_size / 2.7);

        let glyphs = self.text_shaper.shape_text(&text, (width, height));
        let glyphs = glyphs
            .iter()
            .map(|LaidGlyph { glyph_index, x, y }| GlyphInstance {
                index: *glyph_index,
                point: LayoutPoint::new(text_x + x, text_y + y),
            })
            .collect();

        let font_key = FontInstanceKey::new(self.render_api.get_namespace_id(), text.font_size as u32);

        let item = SpecificDisplayItem::Text(TextDisplayItem {
            font_key,
            color: text.color.clone().into(),
            glyph_options: None,
        });

        (item, glyphs)
    }

    fn border(&self, border: Border) -> SpecificDisplayItem {
        SpecificDisplayItem::Border(BorderDisplayItem {
            widths: TypedSideOffsets2D::new(
                border.top.width,
                border.right.width,
                border.bottom.width,
                border.left.width,
            ),
            details: BorderDetails::Normal(NormalBorder {
                top: border.top.into(),
                right: border.right.into(),
                bottom: border.bottom.into(),
                left: border.left.into(),
                radius: self.border_radius.clone().into(),
                do_aa: true,
            }),
        })
    }

    fn push(&mut self, item: SpecificDisplayItem) {
        debug!("push {:?}", &item);

        self.builder
            .push_item(&item, &self.layout, &self.space_and_clip);
    }
}

// unlike browser, we are going to have only one pipeline (per window)
static PIPELINE_ID: PipelineId = PipelineId(0, 0);

static BUILDER_CAPACITY: usize = 512 * 1024;

impl Into<ColorF> for Color {
    fn into(self) -> ColorF {
        let Color(r, g, b, a) = self;
        ColorU::new(r, g, b, a).into()
    }
}

impl Into<LayoutVector2D> for Vector2f {
    fn into(self) -> LayoutVector2D {
        LayoutVector2D::new(self.0, self.1)
    }
}

impl Into<WRBorderRadius> for BorderRadius {
    fn into(self) -> WRBorderRadius {
        WRBorderRadius {
            top_left: LayoutSize::new(self.0, self.0),
            top_right: LayoutSize::new(self.1, self.1),
            bottom_right: LayoutSize::new(self.2, self.2),
            bottom_left: LayoutSize::new(self.3, self.3),
        }
    }
}

impl Into<WRBorderSide> for BorderSide {
    fn into(self) -> WRBorderSide {
        WRBorderSide {
            color: self.color.into(),
            style: self.style.into(),
        }
    }
}

// TODO: more styles
impl Into<WRBorderStyle> for BorderStyle {
    fn into(self) -> WRBorderStyle {
        match self {
            BorderStyle::None => WRBorderStyle::None,
            BorderStyle::Solid => WRBorderStyle::Solid,
        }
    }
}

/*
#[cfg(test)]
mod tests {
    use super::*;
    use crate::generated::Vector2f;

    fn test_ctx() -> SurfaceContext {
        // some "rect", optionally rounded (param to this fn?)

        SurfaceContext {
            border_radius: BorderRadius(0., 0., 0., 0.),
            layout: LayoutPrimitiveInfo::new(LayoutSize::new(100., 100.).into()),
        }
    }

    #[test]
    fn test_background_color() {
        let ctx = test_ctx();
        let color = Color(0, 0, 0, 255);

        assert_eq!(
            ctx.background_color(color.clone()),
            SpecificDisplayItem::Rectangle(RectangleDisplayItem {
                color: color.into()
            })
        );
    }

    #[test]
    fn test_box_shadow() {
        let ctx = test_ctx();
        let box_bounds = LayoutSize::new(100., 100.).into();
        let border_radius = BorderRadius(5., 5., 5., 5.);
        let color = Color(0, 0, 0, 255);
        let blur = 10.;
        let spread = 5.;
        let offset = Vector2f(5., 5.);
        let box_shadow = BoxShadow {
            offset: offset.clone(),
            blur,
            spread,
            color: color.clone(),
        };

        assert_eq!(
            ctx.box_shadow(box_shadow),
            SpecificDisplayItem::BoxShadow(BoxShadowDisplayItem {
                box_bounds,
                offset: offset.into(),
                color: color.into(),
                blur_radius: blur,
                spread_radius: spread,
                border_radius: border_radius.into(),
                clip_mode: BoxShadowClipMode::Outset
            })
        );
    }
}
*/