use crate::api::{App, Event, Window, WindowId};
use crate::window::AppWindow;
use glfw::{Context, Glfw, WindowEvent};
use std::collections::BTreeMap;
use std::sync::mpsc::Receiver;

pub struct TheApp {
    glfw: Glfw,
    windows: BTreeMap<WindowId, (AppWindow, Receiver<(f64, WindowEvent)>)>,
    next_window_id: WindowId,
}

impl TheApp {
    pub fn init() -> Self {
        let mut glfw = glfw::init(glfw::FAIL_ON_ERRORS).expect("could not init GLFW");

        glfw.window_hint(glfw::WindowHint::ContextVersion(3, 2));
        glfw.window_hint(glfw::WindowHint::OpenGlForwardCompat(true));
        glfw.window_hint(glfw::WindowHint::OpenGlProfile(
            glfw::OpenGlProfileHint::Core,
        ));

        TheApp {
            glfw,
            windows: BTreeMap::new(),
            next_window_id: 1,
        }
    }
}

impl App for TheApp {
    fn get_events(&mut self, poll: bool) -> Vec<Event> {
        if poll {
            self.glfw.poll_events()
        } else {
            // wait a bit otherwise (save battery)
            self.glfw.wait_events_timeout(0.1);
        }

        // go through all windows, handle their events, collect all the resulting events and wrap them along with respective window_id
        self.windows
            .iter_mut()
            .flat_map(|(id, (window, events))| {
                glfw::flush_messages(events)
                    .filter_map(move |(_, e)| window.handle_event(e))
                    .map(move |e| Event::WindowEvent {
                        window: *id,
                        event: e,
                    })
            })
            .collect()
    }

    fn create_window(&mut self) -> WindowId {
        let (mut glfw_window, events) = self
            .glfw
            .create_window(1024, 768, "stain", glfw::WindowMode::Windowed)
            .expect("couldnt create GLFW window");

        glfw_window.make_current();
        glfw_window.set_all_polling(true);

        let id = self.next_window_id;
        let window = AppWindow::new(glfw_window);

        self.windows.insert(id, (window, events));

        self.next_window_id = self.next_window_id + 1;

        // vsync off (for now)
        //self.glfw.set_swap_interval(glfw::SwapInterval::None);

        id
    }

    fn get_window_mut(&mut self, id: WindowId) -> &mut Window {
        &mut self.windows.get_mut(&id).expect("window not found").0
    }

    fn destroy_window(&mut self, id: WindowId) {
        self.windows.remove(&id);
    }
}
