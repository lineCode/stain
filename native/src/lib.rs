#[macro_use]
extern crate log;
#[macro_use]
extern crate serde_derive;

use crate::images::make_checkerboard;
use crate::images::ImgJsPayload;
use napi_rs::*;
use std::cell::RefCell;
use std::rc::Rc;
use yoga::{FlexStyle, MeasureMode, Size};

mod images;
mod rendering;
mod resources;
mod surface;
mod window;

use crate::resources::{OpResource, ResourceManager};
use crate::surface::{Measure, Surface};
use crate::window::{Application, EventHandler, Window, WindowEvent};

register_module!(node_webrender, init);

thread_local! {
    static APP: RefCell<Application> = RefCell::new(Application::new());
}

fn init<'env>(mut ctx: ModuleInitContext) -> Result<Option<Value<'env, Object>>> {
    env_logger::init();

    ctx.export("app_loop_a_bit", callback!(app_loop_a_bit));
    ctx.export("window_create", callback!(window_create));
    ctx.export(
        "window_get_glyph_indices_and_advances",
        callback!(window_get_glyph_indices_and_advances),
    );
    ctx.export("window_render_surface", callback!(window_render_surface));
    ctx.export("surface_create", callback!(surface_create));
    ctx.export("surface_update", callback!(surface_update));
    ctx.export("surface_append_child", callback!(surface_append_child));
    ctx.export("surface_insert_before", callback!(surface_insert_before));
    ctx.export("surface_remove_child", callback!(surface_remove_child));
    ctx.export(
        "surface_set_measure_func",
        callback!(surface_set_measure_func),
    );
    ctx.export("surface_mark_dirty", callback!(surface_mark_dirty));
    ctx.export("op_resource_create", callback!(op_resource_create));
    ctx.export("flex_style_create", callback!(flex_style_create));
    ctx.export("registerImage", callback!(register_image));

    Ok(None)
}

fn window_create(ctx: CallContext) -> AnyResult {
    let title = ctx.args[0].to_string();
    let width = ctx.args[1].f64();
    let height = ctx.args[2].f64();
    let event_handler = ctx.args[3].cb(ctx.env);

    let window = APP.with(|app| {
        app.borrow_mut()
            .create_window(title, width, height, Box::new(event_handler))
    });

    let mut wrapper = ctx.env.create_object();
    ctx.env.wrap(&mut wrapper, window)?;

    wrapper.into_result()
}

fn app_loop_a_bit(ctx: CallContext) -> AnyResult {
    unsafe { CURRENT_ENV = Some(ctx.env.borrow_forever()) };

    APP.with(|app| {
        app.borrow_mut().loop_a_bit();
    })
    .into_result()
}

fn window_get_glyph_indices_and_advances(ctx: CallContext) -> AnyResult {
    let window: &mut Rc<RefCell<Window>> = ctx.args[0].unwrap(ctx.env);
    let font_size = ctx.args[1].i32() as u32;
    let str = ctx.args[2].to_string();

    let (glyph_indices, advances) = window
        .borrow()
        .get_glyph_indices_and_advances(font_size, &str);

    let mut res_arr = ctx.env.create_array_with_length(2);
    let mut indices_arr = ctx.env.create_array_with_length(glyph_indices.len());
    let mut advances_arr = ctx.env.create_array_with_length(advances.len());

    for (i, glyph_index) in glyph_indices.iter().enumerate() {
        let num = ctx.env.create_int64(*glyph_index as i64);
        indices_arr.set_index(i, num)?;
    }

    for (i, advance) in advances.iter().enumerate() {
        let num = ctx.env.create_double(*advance as f64);
        advances_arr.set_index(i, num)?;
    }

    res_arr.set_index(0, indices_arr)?;
    res_arr.set_index(1, advances_arr)?;

    res_arr.into_result()
}

fn window_render_surface(ctx: CallContext) -> AnyResult {
    unsafe { CURRENT_ENV = Some(ctx.env.borrow_forever()) };

    let window: &mut Rc<RefCell<Window>> = ctx.args[0].unwrap(ctx.env);
    let surface: &mut Rc<RefCell<Surface>> = ctx.args[1].unwrap(ctx.env);
    let available_width = ctx.args[2].f32();
    let available_height = ctx.args[3].f32();

    surface
        .borrow_mut()
        .calculate_layout(available_width, available_height);

    ResourceManager::with(|rm| {
        window
            .borrow_mut()
            .render(&rm.render_ops, &(surface.borrow()));
    });

    Ok(None)
}

fn surface_create(ctx: CallContext) -> AnyResult {
    let surface = Rc::new(RefCell::new(Surface::new()));
    let mut wrapper = ctx.env.create_object();
    ctx.env.wrap(&mut wrapper, surface)?;
    wrapper.into_result()
}

fn surface_update(ctx: CallContext) -> AnyResult {
    let surface: &mut Rc<RefCell<Surface>> = ctx.args[0].unwrap(ctx.env);
    let mut surface = surface.borrow_mut();

    let brush: Option<&mut Rc<OpResource>> = ctx.args[1].unwrap_opt(ctx.env);
    let clip: Option<&mut Rc<OpResource>> = ctx.args[2].unwrap_opt(ctx.env);
    let layout: Option<&mut Rc<Vec<FlexStyle>>> = ctx.args[3].unwrap_opt(ctx.env);

    surface.set_brush(brush.map(|b| b.clone()));
    surface.set_clip(clip.map(|c| c.clone()));

    if let Some(layout) = layout {
        surface.apply_flex_style(layout.clone());
    }

    Ok(None)
}

fn surface_append_child(ctx: CallContext) -> AnyResult {
    let parent: &mut Rc<RefCell<Surface>> = ctx.args[0].unwrap(ctx.env);
    let child: &mut Rc<RefCell<Surface>> = ctx.args[1].unwrap(ctx.env);

    parent
        .borrow_mut()
        .append_child(child.clone())
        .into_result()
}

fn surface_insert_before(ctx: CallContext) -> AnyResult {
    let parent: &mut Rc<RefCell<Surface>> = ctx.args[0].unwrap(ctx.env);
    let child: &mut Rc<RefCell<Surface>> = ctx.args[1].unwrap(ctx.env);
    let before: &mut Rc<RefCell<Surface>> = ctx.args[2].unwrap(ctx.env);

    parent
        .borrow_mut()
        .insert_before(child.clone(), before.clone())
        .into_result()
}

fn surface_remove_child(ctx: CallContext) -> AnyResult {
    let parent: &mut Rc<RefCell<Surface>> = ctx.args[0].unwrap(ctx.env);
    let child: &mut Rc<RefCell<Surface>> = ctx.args[1].unwrap(ctx.env);

    parent
        .borrow_mut()
        .remove_child(child.clone())
        .into_result()
}

fn surface_set_measure_func(ctx: CallContext) -> AnyResult {
    let surface: &mut Rc<RefCell<Surface>> = ctx.args[0].unwrap(ctx.env);

    surface
        .borrow_mut()
        .set_measure(Box::new(ctx.args[1].cb(ctx.env)))
        .into_result()
}

fn surface_mark_dirty(ctx: CallContext) -> AnyResult {
    let surface: &mut Rc<RefCell<Surface>> = ctx.args[0].unwrap(ctx.env);

    surface.borrow_mut().mark_dirty().into_result()
}

fn op_resource_create(ctx: CallContext) -> AnyResult {
    let ops = serde_json::from_str(&ctx.args[0].to_string()).unwrap();
    let op_resource = ResourceManager::with(|rm| Rc::new(rm.create_op_resource(ops)));
    let mut wrapper = ctx.env.create_object();
    ctx.env.wrap(&mut wrapper, op_resource)?;

    wrapper.into_result()
}

fn flex_style_create(ctx: CallContext) -> AnyResult {
    let data = ctx.args[0].to_string();
    let styles: Rc<Vec<FlexStyle>> = Rc::new(serde_json::from_str(&data).expect("invalid style"));
    let mut wrapper = ctx.env.create_object();

    debug!("style {:?} -> {:?}", &data, &styles);

    ctx.env.wrap(&mut wrapper, styles)?;

    wrapper.into_result()
}

fn register_image(ctx: CallContext) -> AnyResult {
    debug!("register_image");
    let window: &mut Rc<RefCell<Window>> = ctx.args[0].unwrap(ctx.env);
    debug!("unwrapped window");
    let data = ctx.args[1].to_string();

    debug!("ImgJsPayload {:?}", &data);

    let ImgJsPayload { id, size } = serde_json::from_str(&data).unwrap();

    let (desc, img_data) = make_checkerboard(size, size);

    window.borrow_mut().register_image(id, desc, img_data);

    Ok(None)
}

impl EventHandler for Ref<Function> {
    fn handle_event(&mut self, e: WindowEvent) {
        let env = get_env();
        let f = env.get_reference_value(self);
        let payload = env
            .create_string(&(serde_json::to_string(&e).expect("could not serialize")))
            .try_into()
            .unwrap();

        let _ = f.call(None, &[payload]).unwrap();
    }
}

impl Measure for Ref<Function> {
    fn measure(&self, w: f32, wm: MeasureMode, h: f32, hm: MeasureMode) -> Size {
        let env = get_env();
        let f = env.get_reference_value(self);

        let w = env.create_double(w.into());
        let wm = env.create_int64(wm as i64);
        let h = env.create_double(h.into());
        let hm = env.create_int64(hm as i64);

        let args = vec![w, wm, h, hm];
        let args: Vec<Value<Any>> = args.iter().map(|a| a.try_into().unwrap()).collect();
        let res: Value<Object> = f.call(None, &args[..]).unwrap().try_into().unwrap();

        let w: Value<Any> = res.get_named_property("width").unwrap();
        let h: Value<Any> = res.get_named_property("height").unwrap();

        Size {
            width: w.f32(),
            height: h.f32(),
        }
    }
}

static mut CURRENT_ENV: Option<&Env> = None;

fn get_env<'a>() -> &'a Env {
    unsafe { CURRENT_ENV.expect("no env available") }
}

// ---
// utils bellow

trait IntoAnyResult {
    fn into_result(&self) -> AnyResult;
}

impl<'env, T: ValueType> IntoAnyResult for Value<'env, T> {
    fn into_result(&self) -> AnyResult {
        unsafe {
            let any: Value<'env, Any> = self.try_into().unwrap();
            Ok(Some(std::mem::transmute(any)))
        }
    }
}

impl IntoAnyResult for () {
    fn into_result(&self) -> AnyResult {
        Ok(None)
    }
}

trait Helper<'env> {
    fn to_string(&self) -> std::string::String;
    fn f64(&self) -> f64;
    fn f32(&self) -> f32;
    fn i64(&self) -> i64;
    fn i32(&self) -> i32;
    fn unwrap<T: 'static>(&self, env: &'env Env) -> &'env mut T;
    fn unwrap_opt<T: 'static>(&self, env: &'env Env) -> Option<&'env mut T>;
    fn cb(&self, env: &'env Env) -> Ref<Function>;
}

impl<'env> Helper<'env> for Value<'env, Any> {
    fn to_string(&self) -> std::string::String {
        let codepoints: Vec<u16> = self.coerce_to_string().unwrap().into();
        std::string::String::from_utf16(&codepoints[..]).unwrap()
    }

    fn f64(&self) -> f64 {
        self.coerce_to_number().expect("not a number").into()
    }

    fn f32(&self) -> f32 {
        self.f64() as f32
    }

    fn i64(&self) -> i64 {
        self.coerce_to_number().expect("not a number").into()
    }

    fn i32(&self) -> i32 {
        self.i64() as i32
    }

    fn unwrap<T: 'static>(&self, env: &'env Env) -> &'env mut T {
        self.unwrap_opt(env).unwrap()
    }

    fn unwrap_opt<T: 'static>(&self, env: &'env Env) -> Option<&'env mut T> {
        let js_object = self.try_into().ok();
        js_object.map(|o| env.unwrap(&o).unwrap())
    }

    fn cb(&self, env: &'env Env) -> Ref<Function> {
        let f: Value<Function> = self.try_into().unwrap_or_else(|err| unsafe {
            panic!(
                "expected cb, found {:?}, err {:?}",
                self.get_value_type(),
                err
            )
        });

        env.create_reference(&f)
    }
}
