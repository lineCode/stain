use crate::api::{App, Event, WindowId};
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
        let glfw = glfw::init(glfw::FAIL_ON_ERRORS).expect("could not init GLFW");

        TheApp {
            glfw,
            windows: BTreeMap::new(),
            //native_ids: BTreeMap::new(),
            next_window_id: 1,
        }
    }
}

impl App<AppWindow> for TheApp {
    fn get_next_event(&mut self) -> Option<Event> {
        // TODO: poll if we are animating
        // wait a bit otherwise (save battery)
        self.glfw.wait_events_timeout(0.1);

        for (id, (window, events)) in self.windows.iter() {
            if let Ok((_, event)) = events.try_recv() {
                return window
                    .translate_event(event)
                    .map(|event| Event::WindowEvent { window: *id, event });
            }
        }

        None
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

        id
    }

    fn get_window(&mut self, id: WindowId) -> &mut AppWindow {
        &mut self.windows.get_mut(&id).expect("window not found").0
    }

    fn destroy_window(&mut self, id: WindowId) {
        self.windows.remove(&id);
    }
}
