use glam::Vec3;
use winit::event::{ElementState, KeyEvent, MouseButton};
use winit::keyboard::{KeyCode, PhysicalKey};
use winit::window::{CursorGrabMode, Window};

use crate::world::{WORLD_SIZE, World};

const CAMERA_MOUSE_SENSITIVITY: f32 = 0.005;
const CAMERA_MAX_PITCH: f32 = 1.55;
const CAMERA_MOVE_SPEED: f32 = 4.5;
const PLAYER_EYE_HEIGHT: f32 = 1.7;
const PLAYER_RADIUS: f32 = 0.3;
const PLAYER_STEP_HEIGHT: f32 = 0.3;
const GRAVITY: f32 = 20.0;
const JUMP_SPEED: f32 = 7.0;

#[derive(Clone, Copy)]
pub struct Camera {
    pub position: Vec3,
    pub yaw: f32,
    pub pitch: f32,
}

pub struct InputState {
    camera: Camera,
    is_mouse_captured: bool,
    move_forward: bool,
    move_backward: bool,
    move_left: bool,
    move_right: bool,
    jump: bool,
    vertical_velocity: f32,
    on_ground: bool,
    world: World,
}

impl Default for InputState {
    fn default() -> Self {
        let world = World::new(42);
        Self {
            camera: Camera {
                position: world.spawn_position(),
                yaw: 0.0,
                pitch: 0.0,
            },
            is_mouse_captured: false,
            move_forward: false,
            move_backward: false,
            move_left: false,
            move_right: false,
            jump: false,
            vertical_velocity: 0.0,
            on_ground: true,
            world,
        }
    }
}

impl InputState {
    pub fn camera(&self) -> Camera {
        self.camera
    }

    pub fn handle_mouse_input(
        &mut self,
        window: &Window,
        state: ElementState,
        button: MouseButton,
    ) {
        if button == MouseButton::Left && state == ElementState::Pressed {
            self.capture_mouse(window);
        }
    }

    pub fn handle_keyboard_input(&mut self, window: &Window, event: KeyEvent) {
        let PhysicalKey::Code(code) = event.physical_key else {
            return;
        };

        match code {
            KeyCode::Escape if event.state == ElementState::Pressed => self.release_mouse(window),
            KeyCode::KeyW => self.move_forward = event.state == ElementState::Pressed,
            KeyCode::KeyS => self.move_backward = event.state == ElementState::Pressed,
            KeyCode::KeyA => self.move_left = event.state == ElementState::Pressed,
            KeyCode::KeyD => self.move_right = event.state == ElementState::Pressed,
            KeyCode::Space => self.jump = event.state == ElementState::Pressed,
            _ => {}
        }
    }

    pub fn handle_mouse_motion(&mut self, delta_x: f32, delta_y: f32) {
        if !self.is_mouse_captured {
            return;
        }

        self.camera.yaw -= delta_x * CAMERA_MOUSE_SENSITIVITY;
        self.camera.pitch = (self.camera.pitch - delta_y * CAMERA_MOUSE_SENSITIVITY)
            .clamp(-CAMERA_MAX_PITCH, CAMERA_MAX_PITCH);
    }

    pub fn release_mouse(&mut self, window: &Window) {
        let _ = window.set_cursor_grab(CursorGrabMode::None);
        window.set_cursor_visible(true);
        self.is_mouse_captured = false;
    }

    pub fn clear_movement(&mut self) {
        self.move_forward = false;
        self.move_backward = false;
        self.move_left = false;
        self.move_right = false;
        self.jump = false;
    }

    pub fn update_camera_position(&mut self, frame_time: f32) {
        let frame_time = frame_time.min(0.05);
        let forward_amount = self.move_forward as i8 - self.move_backward as i8;
        let right_amount = self.move_right as i8 - self.move_left as i8;

        let (yaw_sin, yaw_cos) = self.camera.yaw.sin_cos();
        let forward = Vec3::new(-yaw_sin, 0.0, -yaw_cos);
        let right = Vec3::new(yaw_cos, 0.0, -yaw_sin);
        let movement = (forward * forward_amount as f32 + right * right_amount as f32)
            .normalize_or_zero()
            * CAMERA_MOVE_SPEED
            * frame_time;
        self.move_horizontal(Vec3::new(movement.x, 0.0, 0.0));
        self.move_horizontal(Vec3::new(0.0, 0.0, movement.z));

        if self.jump && self.on_ground {
            self.vertical_velocity = JUMP_SPEED;
            self.on_ground = false;
        }
        self.vertical_velocity -= GRAVITY * frame_time;
        self.camera.position.y += self.vertical_velocity * frame_time;

        let ground = self.ground_height(self.camera.position.x, self.camera.position.z);
        if self.camera.position.y <= ground + PLAYER_EYE_HEIGHT {
            self.camera.position.y = ground + PLAYER_EYE_HEIGHT;
            self.vertical_velocity = 0.0;
            self.on_ground = true;
        } else {
            self.on_ground = false;
        }
    }

    fn move_horizontal(&mut self, movement: Vec3) {
        if movement == Vec3::ZERO {
            return;
        }

        let mut candidate = self.camera.position + movement;
        candidate.x = candidate.x.clamp(PLAYER_RADIUS, WORLD_SIZE - PLAYER_RADIUS);
        candidate.z = candidate.z.clamp(PLAYER_RADIUS, WORLD_SIZE - PLAYER_RADIUS);
        let feet = self.camera.position.y - PLAYER_EYE_HEIGHT;
        let candidate_ground = self.ground_height(candidate.x, candidate.z);
        let can_step = self.on_ground && candidate_ground <= feet + PLAYER_STEP_HEIGHT;
        if candidate_ground <= feet || can_step {
            self.camera.position.x = candidate.x;
            self.camera.position.z = candidate.z;
            if can_step && candidate_ground > feet {
                self.camera.position.y = candidate_ground + PLAYER_EYE_HEIGHT;
            }
        }
    }

    fn ground_height(&self, x: f32, z: f32) -> f32 {
        [
            (-PLAYER_RADIUS, -PLAYER_RADIUS),
            (-PLAYER_RADIUS, PLAYER_RADIUS),
            (PLAYER_RADIUS, -PLAYER_RADIUS),
            (PLAYER_RADIUS, PLAYER_RADIUS),
        ]
        .into_iter()
        .map(|(offset_x, offset_z)| self.world.height_at(x + offset_x, z + offset_z))
        .fold(0.0, f32::max)
    }

    fn capture_mouse(&mut self, window: &Window) {
        let result = window
            .set_cursor_grab(CursorGrabMode::Locked)
            .or_else(|_| window.set_cursor_grab(CursorGrabMode::Confined));

        if result.is_ok() {
            window.set_cursor_visible(false);
            self.is_mouse_captured = true;
        }
    }
}
