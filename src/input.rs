use glam::Vec3;
use winit::event::{ElementState, KeyEvent, MouseButton};
use winit::keyboard::{KeyCode, PhysicalKey};
use winit::window::{CursorGrabMode, Window};

use crate::world::{VOXEL_SIZE, WORLD_SIZE, World};

const CAMERA_MOUSE_SENSITIVITY: f32 = 0.005;
const CAMERA_MAX_PITCH: f32 = 1.55;
const CAMERA_MOVE_SPEED: f32 = 4.5;
const PLAYER_EYE_HEIGHT: f32 = 1.7;
const PLAYER_RADIUS: f32 = 0.3;
const PLAYER_STEP_HEIGHT: f32 = 0.3;
const COLLISION_EPSILON: f32 = VOXEL_SIZE * 0.01;
const GRAVITY: f32 = 20.0;
const JUMP_SPEED: f32 = 7.0;

#[derive(Clone, Copy)]
pub(crate) struct Camera {
    pub position: Vec3,
    pub yaw: f32,
    pub pitch: f32,
}

impl Camera {
    pub(crate) fn look_direction(self) -> Vec3 {
        let (yaw_sin, yaw_cos) = self.yaw.sin_cos();
        let (pitch_sin, pitch_cos) = self.pitch.sin_cos();
        Vec3::new(-yaw_sin * pitch_cos, pitch_sin, -yaw_cos * pitch_cos)
    }
}

pub(crate) struct InputState {
    camera: Camera,
    is_mouse_captured: bool,
    move_forward: bool,
    move_backward: bool,
    move_left: bool,
    move_right: bool,
    jump: bool,
    vertical_velocity: f32,
    on_ground: bool,
}

impl Default for InputState {
    fn default() -> Self {
        Self {
            camera: Camera {
                position: World::spawn_position(),
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
        }
    }
}

impl InputState {
    pub(crate) fn camera(&self) -> Camera {
        self.camera
    }

    pub(crate) fn handle_mouse_input(
        &mut self,
        window: &Window,
        state: ElementState,
        button: MouseButton,
    ) -> bool {
        if button == MouseButton::Left && state == ElementState::Pressed {
            self.capture_mouse(window);
        }
        button == MouseButton::Right && state == ElementState::Pressed && self.is_mouse_captured
    }

    pub(crate) fn handle_keyboard_input(&mut self, window: &Window, event: KeyEvent) {
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

    pub(crate) fn handle_mouse_motion(&mut self, delta_x: f32, delta_y: f32) {
        if !self.is_mouse_captured {
            return;
        }

        self.camera.yaw -= delta_x * CAMERA_MOUSE_SENSITIVITY;
        self.camera.pitch = (self.camera.pitch - delta_y * CAMERA_MOUSE_SENSITIVITY)
            .clamp(-CAMERA_MAX_PITCH, CAMERA_MAX_PITCH);
    }

    pub(crate) fn release_mouse(&mut self, window: &Window) {
        let _ = window.set_cursor_grab(CursorGrabMode::None);
        window.set_cursor_visible(true);
        self.is_mouse_captured = false;
    }

    pub(crate) fn clear_movement(&mut self) {
        self.move_forward = false;
        self.move_backward = false;
        self.move_left = false;
        self.move_right = false;
        self.jump = false;
    }

    pub(crate) fn update_camera_position(&mut self, frame_time: f32, world: &World) {
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
        self.move_horizontal(Vec3::new(movement.x, 0.0, 0.0), world);
        self.move_horizontal(Vec3::new(0.0, 0.0, movement.z), world);

        if self.jump && self.on_ground {
            self.vertical_velocity = JUMP_SPEED;
            self.on_ground = false;
        }
        let previous_y = self.camera.position.y;
        self.vertical_velocity -= GRAVITY * frame_time;
        self.camera.position.y += self.vertical_velocity * frame_time;

        let previous_feet = previous_y - PLAYER_EYE_HEIGHT;
        let feet = self.camera.position.y - PLAYER_EYE_HEIGHT;
        let ground = self
            .ground_height(world, self.camera.position.x, self.camera.position.z)
            .max(
                world
                    .highest_solid_top(
                        self.camera.position.x - PLAYER_RADIUS,
                        self.camera.position.x + PLAYER_RADIUS,
                        self.camera.position.z - PLAYER_RADIUS,
                        self.camera.position.z + PLAYER_RADIUS,
                        feet,
                        previous_feet + COLLISION_EPSILON,
                    )
                    .unwrap_or(0.0),
            );
        if self.camera.position.y <= ground + PLAYER_EYE_HEIGHT {
            self.camera.position.y = ground + PLAYER_EYE_HEIGHT;
            self.vertical_velocity = 0.0;
            self.on_ground = true;
        } else if self.vertical_velocity > 0.0
            && let Some(ceiling) = world.lowest_solid_bottom(
                self.camera.position.x - PLAYER_RADIUS,
                self.camera.position.x + PLAYER_RADIUS,
                self.camera.position.z - PLAYER_RADIUS,
                self.camera.position.z + PLAYER_RADIUS,
                previous_y,
                self.camera.position.y,
            )
        {
            self.camera.position.y = ceiling - COLLISION_EPSILON;
            self.vertical_velocity = 0.0;
            self.on_ground = false;
        } else {
            self.on_ground = false;
        }
    }

    fn move_horizontal(&mut self, movement: Vec3, world: &World) {
        if movement == Vec3::ZERO {
            return;
        }

        let mut candidate = self.camera.position + movement;
        candidate.x = candidate.x.clamp(PLAYER_RADIUS, WORLD_SIZE - PLAYER_RADIUS);
        candidate.z = candidate.z.clamp(PLAYER_RADIUS, WORLD_SIZE - PLAYER_RADIUS);
        let feet = self.camera.position.y - PLAYER_EYE_HEIGHT;
        let candidate_ground = self.ground_height(world, candidate.x, candidate.z);
        let can_step = self.on_ground && candidate_ground <= feet + PLAYER_STEP_HEIGHT;
        let candidate_feet = if can_step {
            candidate_ground.max(feet)
        } else {
            feet
        };
        let hits_solid_voxel = world.intersects_solid_voxels(
            Vec3::new(
                candidate.x - PLAYER_RADIUS,
                candidate_feet + COLLISION_EPSILON,
                candidate.z - PLAYER_RADIUS,
            ),
            Vec3::new(
                candidate.x + PLAYER_RADIUS,
                candidate_feet + PLAYER_EYE_HEIGHT,
                candidate.z + PLAYER_RADIUS,
            ),
        );
        if (candidate_ground <= feet || can_step) && !hits_solid_voxel {
            self.camera.position.x = candidate.x;
            self.camera.position.z = candidate.z;
            if can_step && candidate_ground > feet {
                self.camera.position.y = candidate_ground + PLAYER_EYE_HEIGHT;
            }
        }
    }

    fn ground_height(&self, world: &World, x: f32, z: f32) -> f32 {
        [
            (-PLAYER_RADIUS, -PLAYER_RADIUS),
            (-PLAYER_RADIUS, PLAYER_RADIUS),
            (PLAYER_RADIUS, -PLAYER_RADIUS),
            (PLAYER_RADIUS, PLAYER_RADIUS),
        ]
        .into_iter()
        .map(|(offset_x, offset_z)| world.height_at(x + offset_x, z + offset_z))
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
