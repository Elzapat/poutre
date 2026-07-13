use std::sync::Arc;
use std::time::Instant;

use winit::application::ApplicationHandler;
use winit::event::{DeviceEvent, DeviceId, ElementState, WindowEvent};
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop};
use winit::keyboard::{KeyCode, PhysicalKey};
use winit::window::{Window, WindowId};

use crate::graphics::Graphics;
use crate::input::InputState;
use crate::network::Network;
use crate::world::World;

pub fn run() {
    tracing_subscriber::fmt::init();

    let event_loop = EventLoop::new().expect("failed to create event loop");
    event_loop.set_control_flow(ControlFlow::Poll);

    let mut app = App::default();
    event_loop.run_app(&mut app).expect("event loop failed");
}

#[derive(Default)]
struct App {
    state: Option<State>,
}

impl ApplicationHandler for App {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.state.is_some() {
            return;
        }

        let window = Arc::new(
            event_loop
                .create_window(Window::default_attributes().with_title("poutre"))
                .expect("failed to create window"),
        );
        let graphics = pollster::block_on(Graphics::new(window.clone()));

        self.state = Some(State::new(window, graphics));
    }

    fn window_event(
        &mut self,
        event_loop: &ActiveEventLoop,
        window_id: WindowId,
        event: WindowEvent,
    ) {
        let Some(state) = self.state.as_mut() else {
            return;
        };

        if state.window.id() != window_id {
            return;
        }

        let egui_response = state.graphics.handle_window_event(&state.window, &event);

        match event {
            WindowEvent::CloseRequested => event_loop.exit(),
            WindowEvent::Resized(size) => state.graphics.resize(size),
            WindowEvent::ScaleFactorChanged { .. } => {
                state.graphics.resize(state.window.inner_size());
            }
            WindowEvent::MouseInput {
                state: button_state,
                button,
                ..
            } => {
                if !egui_response.consumed {
                    let should_excavate =
                        state
                            .input
                            .handle_mouse_input(&state.window, button_state, button);
                    if should_excavate {
                        state.excavate();
                    }
                }
            }
            WindowEvent::KeyboardInput { event, .. } => {
                let is_escape = matches!(event.physical_key, PhysicalKey::Code(KeyCode::Escape));
                if is_escape || event.state == ElementState::Released || !egui_response.consumed {
                    state.input.handle_keyboard_input(&state.window, event);
                }
            }
            WindowEvent::Focused(false) => {
                state.input.release_mouse(&state.window);
                state.input.clear_movement();
            }
            WindowEvent::RedrawRequested => state.render(),
            _ => {}
        }
    }

    fn device_event(
        &mut self,
        _event_loop: &ActiveEventLoop,
        _device_id: DeviceId,
        event: DeviceEvent,
    ) {
        let Some(state) = self.state.as_mut() else {
            return;
        };

        if let DeviceEvent::MouseMotion { delta } = event {
            state
                .input
                .handle_mouse_motion(delta.0 as f32, delta.1 as f32);
        }
    }

    fn about_to_wait(&mut self, _event_loop: &ActiveEventLoop) {
        if let Some(state) = self.state.as_ref() {
            state.window.request_redraw();
        }
    }
}

struct State {
    window: Arc<Window>,
    graphics: Graphics,
    input: InputState,
    network: Network,
    world: World,
    last_frame_at: Instant,
    fps: f32,
}

impl State {
    fn new(window: Arc<Window>, graphics: Graphics) -> Self {
        Self {
            window,
            graphics,
            input: InputState::default(),
            network: Network::connect(),
            world: World::default(),
            last_frame_at: Instant::now(),
            fps: 0.0,
        }
    }

    fn render(&mut self) {
        let now = Instant::now();
        let frame_time = now.duration_since(self.last_frame_at).as_secs_f32();
        self.last_frame_at = now;
        let network_update = self.network.tick(self.input.camera());
        for chunk in network_update.chunks {
            self.world.insert_chunk(
                chunk.id,
                chunk.chunk_x,
                chunk.chunk_z,
                chunk.heights,
                chunk.solid_quads,
                chunk.water_quads,
            );
        }
        self.input.update_camera_position(frame_time, &self.world);
        let camera = self.input.camera();

        if frame_time > 0.0 {
            let instant_fps = 1.0 / frame_time;
            self.fps = if self.fps == 0.0 {
                instant_fps
            } else {
                self.fps * 0.9 + instant_fps * 0.1
            };
        }

        self.graphics.render(
            &self.window,
            camera,
            &self.world,
            &network_update.remote_players,
            self.fps,
        );
    }

    fn excavate(&self) {
        const MAX_DISTANCE: f32 = 8.0;

        let camera = self.input.camera();
        let Some([x, y, z]) =
            self.world
                .raycast_solid(camera.position, camera.look_direction(), MAX_DISTANCE)
        else {
            return;
        };
        self.network.excavate(x, y, z);
    }
}
