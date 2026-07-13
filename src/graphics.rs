use std::sync::{Arc, mpsc};
use std::thread;
use std::time::{Duration, Instant};

use egui_wgpu::ScreenDescriptor;
use winit::dpi::PhysicalSize;
use winit::event::WindowEvent;
use winit::window::Window;

use crate::input::Camera;
use crate::network::RemotePlayer;
use crate::world::{Mesh, MeshRequest, Quad, VOXEL_SIZE, World};

const CAMERA_FOV_Y_DEGREES: f32 = 90.0;

pub struct Graphics {
    surface: wgpu::Surface<'static>,
    device: wgpu::Device,
    queue: wgpu::Queue,
    surface_config: wgpu::SurfaceConfiguration,
    scene_color_view: wgpu::TextureView,
    scene_depth_view: wgpu::TextureView,
    voxel_pipeline: wgpu::RenderPipeline,
    water_pipeline: wgpu::RenderPipeline,
    player_pipeline: wgpu::RenderPipeline,
    composite_pipeline: wgpu::RenderPipeline,
    voxel_quad_buffer: wgpu::Buffer,
    voxel_quad_capacity: usize,
    voxel_quad_count: u32,
    water_quad_buffer: wgpu::Buffer,
    water_quad_capacity: usize,
    water_quad_count: u32,
    voxel_chunk_count: usize,
    player_instance_buffer: wgpu::Buffer,
    player_instance_capacity: usize,
    player_instance_count: u32,
    requested_stream_cell: (isize, isize),
    requested_world_revision: u64,
    last_mesh_request_at: Instant,
    mesh_request_sender: mpsc::Sender<MeshRequest>,
    mesh_result_receiver: mpsc::Receiver<Mesh>,
    camera_bind_group: wgpu::BindGroup,
    camera_uniform_buffer: wgpu::Buffer,
    scene_bind_group_layout: wgpu::BindGroupLayout,
    scene_bind_group: wgpu::BindGroup,
    started_at: Instant,
    egui_context: egui::Context,
    egui_state: egui_winit::State,
    egui_renderer: egui_wgpu::Renderer,
}

impl Graphics {
    pub async fn new(window: Arc<Window>) -> Self {
        let size = window.inner_size();
        let instance = wgpu::Instance::default();
        let surface = instance
            .create_surface(window.clone())
            .expect("failed to create surface");
        let adapter = instance
            .request_adapter(&wgpu::RequestAdapterOptions {
                power_preference: wgpu::PowerPreference::HighPerformance,
                compatible_surface: Some(&surface),
                force_fallback_adapter: false,
            })
            .await
            .expect("failed to find a compatible GPU adapter");
        let (device, queue) = adapter
            .request_device(&wgpu::DeviceDescriptor {
                label: Some("device"),
                required_features: wgpu::Features::empty(),
                required_limits: wgpu::Limits::default(),
                experimental_features: wgpu::ExperimentalFeatures::disabled(),
                memory_hints: wgpu::MemoryHints::Performance,
                trace: wgpu::Trace::Off,
            })
            .await
            .expect("failed to create device");

        let surface_capabilities = surface.get_capabilities(&adapter);
        let mut surface_config = surface
            .get_default_config(&adapter, size.width.max(1), size.height.max(1))
            .expect("surface is not supported by adapter");
        if surface_config.format.is_srgb()
            && let Some(format) = surface_capabilities
                .formats
                .iter()
                .copied()
                .find(|format| !format.is_srgb())
        {
            surface_config.format = format;
        }
        surface_config.present_mode = wgpu::PresentMode::AutoNoVsync;
        surface.configure(&device, &surface_config);

        let (scene_color_view, scene_depth_view) = create_scene_views(
            &device,
            surface_config.width,
            surface_config.height,
            surface_config.format,
        );
        let spawn = World::spawn_position();
        let voxel_quad_buffer = empty_quad_buffer(&device, "voxel_quad_buffer");
        let water_quad_buffer = empty_quad_buffer(&device, "water_quad_buffer");
        let (
            voxel_pipeline,
            water_pipeline,
            player_pipeline,
            composite_pipeline,
            camera_bind_group,
            camera_uniform_buffer,
            scene_bind_group_layout,
            scene_bind_group,
        ) = create_pipelines(
            &device,
            surface_config.format,
            &scene_color_view,
            &scene_depth_view,
        );
        let player_instance_capacity = 16;
        let player_instance_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("player_instance_buffer"),
            size: (player_instance_capacity * std::mem::size_of::<PlayerInstance>()) as u64,
            usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        let egui_context = egui::Context::default();
        let egui_state = egui_winit::State::new(
            egui_context.clone(),
            egui::ViewportId::ROOT,
            window.as_ref(),
            Some(window.scale_factor() as f32),
            window.theme(),
            None,
        );
        let egui_renderer = egui_wgpu::Renderer::new(
            &device,
            surface_config.format,
            egui_wgpu::RendererOptions::default(),
        );
        let (mesh_request_sender, mesh_result_receiver) = spawn_mesh_worker();
        Self {
            surface,
            device,
            queue,
            surface_config,
            scene_color_view,
            scene_depth_view,
            voxel_pipeline,
            water_pipeline,
            player_pipeline,
            composite_pipeline,
            voxel_quad_buffer,
            voxel_quad_capacity: 1,
            voxel_quad_count: 0,
            water_quad_buffer,
            water_quad_capacity: 1,
            water_quad_count: 0,
            voxel_chunk_count: 0,
            player_instance_buffer,
            player_instance_capacity,
            player_instance_count: 0,
            requested_stream_cell: World::stream_cell(spawn),
            requested_world_revision: 0,
            last_mesh_request_at: Instant::now(),
            mesh_request_sender,
            mesh_result_receiver,
            camera_bind_group,
            camera_uniform_buffer,
            scene_bind_group_layout,
            scene_bind_group,
            started_at: Instant::now(),
            egui_context,
            egui_state,
            egui_renderer,
        }
    }

    pub fn handle_window_event(
        &mut self,
        window: &Window,
        event: &WindowEvent,
    ) -> egui_winit::EventResponse {
        self.egui_state.on_window_event(window, event)
    }

    pub fn resize(&mut self, size: PhysicalSize<u32>) {
        if size.width == 0 || size.height == 0 {
            return;
        }

        self.surface_config.width = size.width;
        self.surface_config.height = size.height;
        self.surface.configure(&self.device, &self.surface_config);
        (self.scene_color_view, self.scene_depth_view) = create_scene_views(
            &self.device,
            size.width,
            size.height,
            self.surface_config.format,
        );
        self.scene_bind_group = create_scene_bind_group(
            &self.device,
            &self.scene_bind_group_layout,
            &self.scene_color_view,
            &self.scene_depth_view,
        );
    }

    pub fn render(
        &mut self,
        window: &Window,
        camera: Camera,
        world: &World,
        remote_players: &[RemotePlayer],
        fps: f32,
    ) {
        if self.surface_config.width == 0 || self.surface_config.height == 0 {
            return;
        }

        self.update_streamed_mesh(camera, world);
        self.update_camera_uniforms(camera);
        self.update_player_instances(remote_players);

        let surface_texture = match self.surface.get_current_texture() {
            wgpu::CurrentSurfaceTexture::Success(surface_texture)
            | wgpu::CurrentSurfaceTexture::Suboptimal(surface_texture) => surface_texture,
            wgpu::CurrentSurfaceTexture::Lost | wgpu::CurrentSurfaceTexture::Outdated => {
                self.resize(window.inner_size());
                return;
            }
            wgpu::CurrentSurfaceTexture::Timeout
            | wgpu::CurrentSurfaceTexture::Occluded
            | wgpu::CurrentSurfaceTexture::Validation => return,
        };

        let view = surface_texture
            .texture
            .create_view(&wgpu::TextureViewDescriptor::default());
        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("main_encoder"),
            });

        let raw_input = self.egui_state.take_egui_input(window);
        let full_output = self.egui_context.run_ui(raw_input, |ui| {
            egui::Window::new("Stats")
                .anchor(egui::Align2::LEFT_TOP, [12.0, 12.0])
                .resizable(false)
                .collapsible(false)
                .show(ui.ctx(), |ui| {
                    ui.label(format!("FPS: {:.1}", fps));
                    ui.label(format!("Visible chunks: {}", self.voxel_chunk_count));
                    ui.label(format!(
                        "Voxel quads: {}",
                        self.voxel_quad_count + self.water_quad_count
                    ));
                    ui.label(format!("Voxel size: {:.1}", VOXEL_SIZE));
                    ui.label(format!(
                        "Triangles: {}",
                        (self.voxel_quad_count + self.water_quad_count) * 2
                    ));
                    ui.label(format!("Remote players: {}", self.player_instance_count));
                    ui.separator();
                    ui.label("Click to look");
                    ui.label("WASD to move, Space to jump");
                });
        });
        self.egui_state
            .handle_platform_output(window, full_output.platform_output);

        for (id, image_delta) in &full_output.textures_delta.set {
            self.egui_renderer
                .update_texture(&self.device, &self.queue, *id, image_delta);
        }

        let pixels_per_point = full_output.pixels_per_point;
        let paint_jobs = self
            .egui_context
            .tessellate(full_output.shapes, pixels_per_point);
        let screen_descriptor = ScreenDescriptor {
            size_in_pixels: [self.surface_config.width, self.surface_config.height],
            pixels_per_point,
        };
        let callback_commands = self.egui_renderer.update_buffers(
            &self.device,
            &self.queue,
            &mut encoder,
            &paint_jobs,
            &screen_descriptor,
        );

        {
            let mut render_pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("voxel_pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &self.scene_color_view,
                    resolve_target: None,
                    depth_slice: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color {
                            r: 0.42,
                            g: 0.68,
                            b: 0.92,
                            a: 1.0,
                        }),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                    view: &self.scene_depth_view,
                    depth_ops: Some(wgpu::Operations {
                        load: wgpu::LoadOp::Clear(1.0),
                        store: wgpu::StoreOp::Store,
                    }),
                    stencil_ops: None,
                }),
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            render_pass.set_pipeline(&self.voxel_pipeline);
            render_pass.set_bind_group(0, &self.camera_bind_group, &[]);
            render_pass.set_vertex_buffer(0, self.voxel_quad_buffer.slice(..));
            render_pass.draw(0..4, 0..self.voxel_quad_count);
            render_pass.set_pipeline(&self.player_pipeline);
            render_pass.set_vertex_buffer(0, self.player_instance_buffer.slice(..));
            render_pass.draw(0..36, 0..self.player_instance_count);
        }

        {
            let mut render_pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("water_composite_pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &view,
                    resolve_target: None,
                    depth_slice: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            render_pass.set_pipeline(&self.composite_pipeline);
            render_pass.set_bind_group(0, &self.scene_bind_group, &[]);
            render_pass.draw(0..3, 0..1);
            render_pass.set_pipeline(&self.water_pipeline);
            render_pass.set_bind_group(0, &self.camera_bind_group, &[]);
            render_pass.set_bind_group(1, &self.scene_bind_group, &[]);
            render_pass.set_vertex_buffer(0, self.water_quad_buffer.slice(..));
            render_pass.draw(0..4, 0..self.water_quad_count);
        }

        {
            let render_pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("egui_pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &view,
                    resolve_target: None,
                    depth_slice: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Load,
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            self.egui_renderer.render(
                &mut render_pass.forget_lifetime(),
                &paint_jobs,
                &screen_descriptor,
            );
        }

        for id in &full_output.textures_delta.free {
            self.egui_renderer.free_texture(id);
        }

        self.queue.submit(callback_commands);
        self.queue.submit(Some(encoder.finish()));
        surface_texture.present();
    }

    fn update_camera_uniforms(&self, camera: Camera) {
        let aspect = self.surface_config.width as f32 / self.surface_config.height.max(1) as f32;
        let focal_length = 1.0 / (CAMERA_FOV_Y_DEGREES.to_radians() * 0.5).tan();
        let mut bytes = [0_u8; 32];
        bytes[0..4].copy_from_slice(&aspect.to_ne_bytes());
        bytes[4..8].copy_from_slice(&focal_length.to_ne_bytes());
        bytes[8..12].copy_from_slice(&camera.yaw.to_ne_bytes());
        bytes[12..16].copy_from_slice(&camera.pitch.to_ne_bytes());
        bytes[16..20].copy_from_slice(&camera.position.x.to_ne_bytes());
        bytes[20..24].copy_from_slice(&camera.position.y.to_ne_bytes());
        bytes[24..28].copy_from_slice(&camera.position.z.to_ne_bytes());
        bytes[28..32].copy_from_slice(&self.started_at.elapsed().as_secs_f32().to_ne_bytes());
        self.queue
            .write_buffer(&self.camera_uniform_buffer, 0, &bytes);
    }

    fn update_streamed_mesh(&mut self, camera: Camera, world: &World) {
        let mut latest_mesh = None;
        while let Ok(mesh) = self.mesh_result_receiver.try_recv() {
            latest_mesh = Some(mesh);
        }
        if let Some(mesh) = latest_mesh {
            upload_quad_buffer(
                &self.device,
                &self.queue,
                &mut self.voxel_quad_buffer,
                &mut self.voxel_quad_capacity,
                "voxel_quad_buffer",
                &mesh.quads,
            );
            upload_quad_buffer(
                &self.device,
                &self.queue,
                &mut self.water_quad_buffer,
                &mut self.water_quad_capacity,
                "water_quad_buffer",
                &mesh.water_quads,
            );
            self.voxel_quad_count = mesh.quads.len() as u32;
            self.water_quad_count = mesh.water_quads.len() as u32;
            self.voxel_chunk_count = mesh.chunk_count;
        }

        let streamed_cell = World::stream_cell(camera.position);
        let stream_cell_changed = streamed_cell != self.requested_stream_cell;
        let world_changed = world.revision() != self.requested_world_revision;
        if !stream_cell_changed && !world_changed {
            return;
        }
        if !stream_cell_changed
            && self.voxel_chunk_count > 0
            && self.last_mesh_request_at.elapsed() < Duration::from_millis(250)
        {
            return;
        }

        if self
            .mesh_request_sender
            .send(world.mesh_request(camera.position))
            .is_ok()
        {
            self.requested_stream_cell = streamed_cell;
            self.requested_world_revision = world.revision();
            self.last_mesh_request_at = Instant::now();
        }
    }

    fn update_player_instances(&mut self, players: &[RemotePlayer]) {
        let instances: Vec<_> = players
            .iter()
            .map(|player| PlayerInstance {
                position: [player.position.x, player.position.y, player.position.z, 0.0],
                facing: [player.yaw, 0.0, 0.0, 0.0],
            })
            .collect();

        if instances.len() > self.player_instance_capacity {
            self.player_instance_capacity = instances.len().next_power_of_two();
            self.player_instance_buffer = self.device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("player_instance_buffer"),
                size: (self.player_instance_capacity * std::mem::size_of::<PlayerInstance>())
                    as u64,
                usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            });
        }
        if !instances.is_empty() {
            self.queue.write_buffer(
                &self.player_instance_buffer,
                0,
                player_instance_bytes(&instances),
            );
        }
        self.player_instance_count = instances.len() as u32;
    }
}

#[derive(Clone, Copy)]
#[repr(C)]
struct PlayerInstance {
    position: [f32; 4],
    facing: [f32; 4],
}

fn spawn_mesh_worker() -> (mpsc::Sender<MeshRequest>, mpsc::Receiver<Mesh>) {
    let (request_sender, request_receiver) = mpsc::channel::<MeshRequest>();
    let (result_sender, result_receiver) = mpsc::sync_channel(1);
    thread::Builder::new()
        .name("terrain-mesher".into())
        .spawn(move || {
            while let Ok(mut request) = request_receiver.recv() {
                while let Ok(newer_request) = request_receiver.try_recv() {
                    request = newer_request;
                }
                if result_sender.send(request.build()).is_err() {
                    break;
                }
            }
        })
        .expect("failed to start terrain mesher");
    (request_sender, result_receiver)
}

fn create_pipelines(
    device: &wgpu::Device,
    color_format: wgpu::TextureFormat,
    scene_color_view: &wgpu::TextureView,
    scene_depth_view: &wgpu::TextureView,
) -> (
    wgpu::RenderPipeline,
    wgpu::RenderPipeline,
    wgpu::RenderPipeline,
    wgpu::RenderPipeline,
    wgpu::BindGroup,
    wgpu::Buffer,
    wgpu::BindGroupLayout,
    wgpu::BindGroup,
) {
    let shader = device.create_shader_module(wgpu::include_wgsl!("shaders/cube.wgsl"));

    let uniform_buffer = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("camera_uniform_buffer"),
        size: 32,
        usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });

    let bind_group_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some("camera_bind_group_layout"),
        entries: &[wgpu::BindGroupLayoutEntry {
            binding: 0,
            visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
            ty: wgpu::BindingType::Buffer {
                ty: wgpu::BufferBindingType::Uniform,
                has_dynamic_offset: false,
                min_binding_size: None,
            },
            count: None,
        }],
    });

    let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("camera_bind_group"),
        layout: &bind_group_layout,
        entries: &[wgpu::BindGroupEntry {
            binding: 0,
            resource: uniform_buffer.as_entire_binding(),
        }],
    });

    let scene_bind_group_layout =
        device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("scene_bind_group_layout"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Depth,
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 2,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
            ],
        });
    let scene_bind_group = create_scene_bind_group(
        device,
        &scene_bind_group_layout,
        scene_color_view,
        scene_depth_view,
    );

    let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: Some("cube_pipeline_layout"),
        bind_group_layouts: &[Some(&bind_group_layout)],
        immediate_size: 0,
    });

    let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some("cube_pipeline"),
        layout: Some(&pipeline_layout),
        vertex: wgpu::VertexState {
            module: &shader,
            entry_point: Some("vs_main"),
            compilation_options: wgpu::PipelineCompilationOptions::default(),
            buffers: &[wgpu::VertexBufferLayout {
                array_stride: 16,
                step_mode: wgpu::VertexStepMode::Instance,
                attributes: &wgpu::vertex_attr_array![0 => Uint32x4],
            }],
        },
        primitive: wgpu::PrimitiveState {
            topology: wgpu::PrimitiveTopology::TriangleStrip,
            ..Default::default()
        },
        depth_stencil: Some(wgpu::DepthStencilState {
            format: wgpu::TextureFormat::Depth32Float,
            depth_write_enabled: Some(true),
            depth_compare: Some(wgpu::CompareFunction::Less),
            stencil: wgpu::StencilState::default(),
            bias: wgpu::DepthBiasState::default(),
        }),
        multisample: wgpu::MultisampleState::default(),
        fragment: Some(wgpu::FragmentState {
            module: &shader,
            entry_point: Some("fs_main"),
            compilation_options: wgpu::PipelineCompilationOptions::default(),
            targets: &[Some(wgpu::ColorTargetState {
                format: color_format,
                blend: Some(wgpu::BlendState::REPLACE),
                write_mask: wgpu::ColorWrites::ALL,
            })],
        }),
        multiview_mask: None,
        cache: None,
    });

    let water_shader = device.create_shader_module(wgpu::include_wgsl!("shaders/water.wgsl"));
    let water_pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: Some("water_pipeline_layout"),
        bind_group_layouts: &[Some(&bind_group_layout), Some(&scene_bind_group_layout)],
        immediate_size: 0,
    });
    let water_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some("water_pipeline"),
        layout: Some(&water_pipeline_layout),
        vertex: wgpu::VertexState {
            module: &water_shader,
            entry_point: Some("vs_main"),
            compilation_options: wgpu::PipelineCompilationOptions::default(),
            buffers: &[wgpu::VertexBufferLayout {
                array_stride: 16,
                step_mode: wgpu::VertexStepMode::Instance,
                attributes: &wgpu::vertex_attr_array![0 => Uint32x4],
            }],
        },
        primitive: wgpu::PrimitiveState {
            topology: wgpu::PrimitiveTopology::TriangleStrip,
            ..Default::default()
        },
        depth_stencil: None,
        multisample: wgpu::MultisampleState::default(),
        fragment: Some(wgpu::FragmentState {
            module: &water_shader,
            entry_point: Some("fs_main"),
            compilation_options: wgpu::PipelineCompilationOptions::default(),
            targets: &[Some(wgpu::ColorTargetState {
                format: color_format,
                blend: Some(wgpu::BlendState::ALPHA_BLENDING),
                write_mask: wgpu::ColorWrites::ALL,
            })],
        }),
        multiview_mask: None,
        cache: None,
    });

    let player_shader = device.create_shader_module(wgpu::include_wgsl!("shaders/player.wgsl"));
    let player_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some("player_pipeline"),
        layout: Some(&pipeline_layout),
        vertex: wgpu::VertexState {
            module: &player_shader,
            entry_point: Some("vs_main"),
            compilation_options: wgpu::PipelineCompilationOptions::default(),
            buffers: &[wgpu::VertexBufferLayout {
                array_stride: std::mem::size_of::<PlayerInstance>() as u64,
                step_mode: wgpu::VertexStepMode::Instance,
                attributes: &wgpu::vertex_attr_array![0 => Float32x4, 1 => Float32x4],
            }],
        },
        primitive: wgpu::PrimitiveState::default(),
        depth_stencil: Some(wgpu::DepthStencilState {
            format: wgpu::TextureFormat::Depth32Float,
            depth_write_enabled: Some(true),
            depth_compare: Some(wgpu::CompareFunction::Less),
            stencil: wgpu::StencilState::default(),
            bias: wgpu::DepthBiasState::default(),
        }),
        multisample: wgpu::MultisampleState::default(),
        fragment: Some(wgpu::FragmentState {
            module: &player_shader,
            entry_point: Some("fs_main"),
            compilation_options: wgpu::PipelineCompilationOptions::default(),
            targets: &[Some(wgpu::ColorTargetState {
                format: color_format,
                blend: Some(wgpu::BlendState::REPLACE),
                write_mask: wgpu::ColorWrites::ALL,
            })],
        }),
        multiview_mask: None,
        cache: None,
    });

    let composite_shader =
        device.create_shader_module(wgpu::include_wgsl!("shaders/composite.wgsl"));
    let composite_pipeline_layout =
        device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("composite_pipeline_layout"),
            bind_group_layouts: &[Some(&scene_bind_group_layout)],
            immediate_size: 0,
        });
    let composite_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some("composite_pipeline"),
        layout: Some(&composite_pipeline_layout),
        vertex: wgpu::VertexState {
            module: &composite_shader,
            entry_point: Some("vs_main"),
            compilation_options: wgpu::PipelineCompilationOptions::default(),
            buffers: &[],
        },
        primitive: wgpu::PrimitiveState::default(),
        depth_stencil: None,
        multisample: wgpu::MultisampleState::default(),
        fragment: Some(wgpu::FragmentState {
            module: &composite_shader,
            entry_point: Some("fs_main"),
            compilation_options: wgpu::PipelineCompilationOptions::default(),
            targets: &[Some(wgpu::ColorTargetState {
                format: color_format,
                blend: Some(wgpu::BlendState::REPLACE),
                write_mask: wgpu::ColorWrites::ALL,
            })],
        }),
        multiview_mask: None,
        cache: None,
    });

    (
        pipeline,
        water_pipeline,
        player_pipeline,
        composite_pipeline,
        bind_group,
        uniform_buffer,
        scene_bind_group_layout,
        scene_bind_group,
    )
}

fn quad_bytes(quads: &[Quad]) -> &[u8] {
    // Quad is repr(C) and consists solely of four u32 values, so it has no padding.
    unsafe { std::slice::from_raw_parts(quads.as_ptr().cast::<u8>(), std::mem::size_of_val(quads)) }
}

fn empty_quad_buffer(device: &wgpu::Device, label: &str) -> wgpu::Buffer {
    device.create_buffer(&wgpu::BufferDescriptor {
        label: Some(label),
        size: std::mem::size_of::<Quad>() as u64,
        usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    })
}

fn upload_quad_buffer(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    buffer: &mut wgpu::Buffer,
    capacity: &mut usize,
    label: &str,
    quads: &[Quad],
) {
    if quads.len() > *capacity {
        *capacity = quads.len().next_power_of_two();
        *buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some(label),
            size: (*capacity * std::mem::size_of::<Quad>()) as u64,
            usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
    }
    if !quads.is_empty() {
        queue.write_buffer(buffer, 0, quad_bytes(quads));
    }
}

fn player_instance_bytes(instances: &[PlayerInstance]) -> &[u8] {
    // PlayerInstance is repr(C) and consists solely of two f32 arrays, so it has no padding.
    unsafe {
        std::slice::from_raw_parts(
            instances.as_ptr().cast::<u8>(),
            std::mem::size_of_val(instances),
        )
    }
}

fn create_scene_views(
    device: &wgpu::Device,
    width: u32,
    height: u32,
    color_format: wgpu::TextureFormat,
) -> (wgpu::TextureView, wgpu::TextureView) {
    let size = wgpu::Extent3d {
        width: width.max(1),
        height: height.max(1),
        depth_or_array_layers: 1,
    };
    let color = device
        .create_texture(&wgpu::TextureDescriptor {
            label: Some("scene_color_texture"),
            size,
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: color_format,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
            view_formats: &[],
        })
        .create_view(&wgpu::TextureViewDescriptor::default());
    let depth = device
        .create_texture(&wgpu::TextureDescriptor {
            label: Some("scene_depth_texture"),
            size,
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Depth32Float,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
            view_formats: &[],
        })
        .create_view(&wgpu::TextureViewDescriptor::default());
    (color, depth)
}

fn create_scene_bind_group(
    device: &wgpu::Device,
    layout: &wgpu::BindGroupLayout,
    color_view: &wgpu::TextureView,
    depth_view: &wgpu::TextureView,
) -> wgpu::BindGroup {
    let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
        label: Some("scene_sampler"),
        address_mode_u: wgpu::AddressMode::ClampToEdge,
        address_mode_v: wgpu::AddressMode::ClampToEdge,
        mag_filter: wgpu::FilterMode::Linear,
        min_filter: wgpu::FilterMode::Linear,
        ..Default::default()
    });
    device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("scene_bind_group"),
        layout,
        entries: &[
            wgpu::BindGroupEntry {
                binding: 0,
                resource: wgpu::BindingResource::TextureView(color_view),
            },
            wgpu::BindGroupEntry {
                binding: 1,
                resource: wgpu::BindingResource::TextureView(depth_view),
            },
            wgpu::BindGroupEntry {
                binding: 2,
                resource: wgpu::BindingResource::Sampler(&sampler),
            },
        ],
    })
}
