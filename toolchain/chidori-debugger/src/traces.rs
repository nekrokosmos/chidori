//! A shader that renders a mesh multiple times in one draw call.

use std::collections::HashMap;
use std::num::NonZero;
use bevy::input::touchpad::TouchpadMagnify;
use std::ops::Add;
use std::time::Instant;
use bevy::{
    prelude::*,
    render::{
        extract_component::ExtractComponent


        ,
        render_phase::{
            PhaseItem, RenderCommand
            ,
        }
        ,
        render_resource::*
        , view::NoFrustumCulling,
    },
};
use bevy::input::mouse::{MouseMotion, MouseWheel};
use bevy::math::{vec2, vec3};
use bevy::render::camera::{ScalingMode, Viewport};
use bevy::render::view::RenderLayers;
use bevy::sprite::Anchor;
use bevy::text::{BreakLineOn, Text2dBounds};
use bevy::utils::petgraph::visit::Walker;
use bevy::window::{PrimaryWindow, WindowResized};
use bevy_egui::{egui, EguiContexts};
use bevy_rapier2d::geometry::{Collider, Sensor};
use bevy_rapier2d::pipeline::QueryFilter;
use bevy_rapier2d::plugin::RapierContext;
use egui_tiles::Tile;
use petgraph::prelude::{EdgeRef, NodeIndex, StableDiGraph, StableGraph};
use chidori_core::utils::telemetry::TraceEvents;
use crate::chidori::{ChidoriTraceEvents, EguiTree};
use crate::shader_trace::{CustomMaterialPlugin, InstanceData, InstanceMaterialData};
use crate::util::despawn_screen;


const RENDER_TEXT: bool = true;
const HANDLE_COLLISIONS: bool = true;
const SPAN_HEIGHT: f32 = 20.0;
const CAMERA_SPACE_WIDTH: f32 = 1000.0;
const MINIMAP_OFFSET: u32 = 0;
const MINIMAP_HEIGHT: u32 = 100;
const MINIMAP_HEIGHT_AND_OFFSET: u32 = MINIMAP_OFFSET + MINIMAP_HEIGHT;

#[derive(Component)]
struct MinimapTraceViewport;

#[derive(Component)]
struct IdentifiedSpan {
    node_idx: NodeIndex,
    id: String,
    is_hovered: bool,
}

#[derive(Resource, Debug)]
struct TraceSpaceViewport {
    x: f32,
    y: f32,
    horizontal_scale: f32, // scale of the view
    vertical_scale: f32,
    max_vertical_extent: f32
}

fn update_trace_space_to_minimap_camera_configuration(
    trace_space: Res<TraceSpaceViewport>,
    mut camera: Query<(&mut Projection, &mut Transform), (With<OnTraceScreen>, With<TraceCameraMinimap>)>,
) {
    let (projection, mut camera_transform) = camera.single_mut();
    let (mut scale) = match projection.into_inner() {
        Projection::Perspective(_) => { unreachable!("This should be orthographic") }
        Projection::Orthographic(ref mut o) => { (&mut o.scaling_mode) }
    };
    camera_transform.translation.y = -trace_space.max_vertical_extent / 2.0;
    *scale = ScalingMode::Fixed {
        width: CAMERA_SPACE_WIDTH,
        height: trace_space.max_vertical_extent,
    };
}

fn fract(x: f32) -> f32 {
    x - x.floor()
}

fn triangle_wave(x: f32) -> f32 {
    2.0 * (fract(x) - 0.5).abs() - 1.0
}

fn color_for_bucket(t: f32, a: f32) -> Color {
    let C_0 = 0.2;
    let C_d = 0.1;
    let L_0 = 0.2;
    let L_d = 0.1;
    let x = triangle_wave(30.0 * t);
    let H = 360.0 * (0.9 * t);
    let C = C_0 + C_d * x;
    let L = L_0 - L_d * x;
    Color::Lcha {
        lightness: L,
        chroma:C,
        hue:H,
        alpha: a,
    }
}


fn update_trace_space_to_camera_configuration(
    windows: Query<&Window>,
    mut trace_space: ResMut<TraceSpaceViewport>,
    mut main_camera: Query<(&mut Projection, &mut Transform), (With<TraceCameraTraces>, Without<TraceCameraMinimap>, Without<TraceCameraTextAtlas>)>,
    mut minimap_camera: Query<(&mut Projection, &mut Transform), (With<TraceCameraMinimap>, Without<TraceCameraTraces>, Without<TraceCameraTextAtlas>)>,
    mut minimap_trace_viewport: Query<(&mut Transform), (With<MinimapTraceViewport>, Without<TraceCameraTraces>, Without<TraceCameraMinimap>)>,
) {

    let window = windows.single();
    let scale_factor = window.scale_factor();
    let span_height = SPAN_HEIGHT * scale_factor;
    let minimap_height_and_offset = MINIMAP_HEIGHT_AND_OFFSET * scale_factor as u32;
    let minimap_offset = (MINIMAP_OFFSET * scale_factor as u32) as f32;
    let (trace_projection, mut trace_camera_transform) = main_camera.single_mut();
    let (mini_projection, mut mini_camera_transform) = minimap_camera.single_mut();

    let trace_projection = match trace_projection.into_inner() {
        Projection::Perspective(_) => { unreachable!("This should be orthographic") }
        Projection::Orthographic(ref mut o) => { o }
    };
    let mini_projection = match mini_projection.into_inner() {
        Projection::Perspective(_) => { unreachable!("This should be orthographic") }
        Projection::Orthographic(ref mut o) => { o }
    };

    trace_projection.scaling_mode = ScalingMode::Fixed {
        width: trace_space.horizontal_scale,
        height: trace_space.vertical_scale,
    };

    let camera_position = mini_camera_transform.translation;
    let trace_viewport_width = trace_projection.area.width();
    let trace_viewport_height = trace_projection.area.height();
    let viewport_width = mini_projection.area.width();
    let viewport_height = mini_projection.area.height().max(trace_viewport_height);

    let left = camera_position.x - viewport_width / 2.0 + (trace_viewport_width / 2.0);
    let right = camera_position.x + viewport_width / 2.0 - (trace_viewport_width / 2.0);
    let top = 0.0;
    let bottom = (-trace_space.max_vertical_extent + (trace_viewport_height)).min(0.0);

    trace_space.x = trace_space.x.clamp(left, right);
    trace_space.y = trace_space.y.clamp(bottom, top);

    trace_camera_transform.translation.x = trace_space.x;
    trace_camera_transform.translation.y = trace_space.y - (trace_space.vertical_scale * 0.5);
    minimap_trace_viewport.iter_mut().for_each(|mut transform| {
        transform.translation.x = trace_space.x;
        transform.translation.y = trace_camera_transform.translation.y;
        transform.scale.x = trace_space.horizontal_scale;
        transform.scale.y = trace_space.vertical_scale;
    });
}



fn mouse_scroll_events(
    mut scroll_evr: EventReader<MouseWheel>,
    mut trace_space: ResMut<TraceSpaceViewport>,
    keyboard_input: Res<ButtonInput<KeyCode>>,
) {
    for ev in scroll_evr.read() {
        if keyboard_input.pressed(KeyCode::SuperLeft) {
            trace_space.horizontal_scale = (trace_space.horizontal_scale + ev.y).clamp(1.0, 1000.0);
        } else {
            trace_space.x -= (ev.x * (trace_space.horizontal_scale / 1000.0));
            trace_space.y += ev.y;
        }
    }
}


#[derive(Component, Default)]
struct CursorWorldCoords(Vec2);




fn my_cursor_system(
    mut q_mycoords: Query<&mut CursorWorldCoords, With<OnTraceScreen>>,
    q_window: Query<&Window, With<PrimaryWindow>>,
    q_camera: Query<(&Camera, &GlobalTransform), (With<OnTraceScreen>, With<TraceCameraTextAtlas>)>,
) {
    let mut coords = q_mycoords.single_mut();
    let (camera, camera_transform) = q_camera.single();
    let window = q_window.single();
    let scale_factor = window.scale_factor();
    let viewport_pos = if let Some(viewport) = &camera.viewport {
        vec2(viewport.physical_position.x as f32 / scale_factor , viewport.physical_position.y as f32 / scale_factor)
    } else {
        Vec2::ZERO
    };
    if let Some(world_position) = window.cursor_position()
        .and_then(|cursor| {
            let adjusted_cursor = cursor - viewport_pos;
            camera.viewport_to_world(camera_transform, adjusted_cursor)
        })
        .map(|ray| ray.origin.truncate())
    {
        // Adjust according to the ratio of our actual window size and our scaling independently of it
        coords.0 = world_position;
    }
}

fn mouse_pan(
    mut trace_space: ResMut<TraceSpaceViewport>,
    buttons: Res<ButtonInput<MouseButton>>,
    mut motion_evr: EventReader<MouseMotion>,
) {
    if buttons.pressed(MouseButton::Left) {
        for ev in motion_evr.read() {
            trace_space.x -= ev.delta.x;
        }
    }
}


// these only work on macOS
fn touchpad_gestures(
    mut trace_space: ResMut<TraceSpaceViewport>,
    mut evr_touchpad_magnify: EventReader<TouchpadMagnify>,
) {
    for ev_magnify in evr_touchpad_magnify.read() {
        trace_space.horizontal_scale = (trace_space.horizontal_scale + (ev_magnify.0 * trace_space.horizontal_scale)).clamp(1.0, 1000.0);
    }
}

fn mouse_over_system(
    q_mycoords: Query<&CursorWorldCoords, With<OnTraceScreen>>,
    mut node_query: Query<(Entity, &Collider, &mut IdentifiedSpan), With<IdentifiedSpan>>,
    mut gizmos: Gizmos,
    rapier_context: Res<RapierContext>,
    mut contexts: EguiContexts,
    call_tree: Res<TracesCallTree>
) {
    let ctx = contexts.ctx_mut();
    let cursor = q_mycoords.single();

    for (_, collider, mut span) in node_query.iter_mut() {
        span.is_hovered = false;
    }

    gizmos
        .circle(Vec3::new(cursor.0.x, cursor.0.y, 0.0), Direction3d::Z, 1.0, Color::YELLOW)
        .segments(64);
    let point = Vec2::new(cursor.0.x, cursor.0.y);
    let filter = QueryFilter::default();
    rapier_context.intersections_with_point(
        point, filter, |entity| {
            if let Ok((_, _, mut span)) = node_query.get_mut(entity) {
                span.is_hovered = true;
                egui::containers::popup::show_tooltip_at_pointer(ctx, egui::Id::new("my_tooltip"), |ui| {
                    call_tree.inner.graph.node_weight(span.node_idx).map(|node| {
                        match &node.event {
                            TraceEvents::NewSpan {name, location, line, thread_id, parent_id, ..} => {
                                ui.label(format!("{:?}", node.id));
                                ui.label(format!("name: {}", name));
                                ui.label(format!("location: {}", location));
                                ui.label(format!("line: {}", line));
                                ui.label(format!("thread_id: {}", thread_id));
                                ui.label(format!("parent_id: {:?}", parent_id));
                                ui.label(format!("absolute_timestamp: {:?}", node.absolute_timestamp));
                                ui.label(format!("parent_relative_timestamp: {:?}", node.adjusted_timestamp));
                            }
                            _ => {}
                        }
                    });
                });
                // dbg!(&span.id);
            }
            false
        }
    );
}


#[derive(Debug, Clone)]
struct CallNode {
    id: String,
    created_at: Instant,
    depth: usize,
    thread_depth: usize,
    absolute_timestamp: u128,
    adjusted_timestamp: u128,
    total_duration: u128,
    event: TraceEvents,
    color_bucket: f32
}

#[derive(Debug)]
struct CallTree {
    max_thread_depth: usize,
    startpoint: u128,
    endpoint: u128,
    relative_endpoint: u128,
    graph: StableGraph<CallNode, ()>
}

impl Default for CallTree {
    fn default() -> Self {
        Self {
            max_thread_depth: 0,
            startpoint: 0,
            endpoint: 0,
            relative_endpoint: 0,
            graph: StableGraph::new()
        }
    }

}


fn build_call_tree(events: Vec<TraceEvents>, collapse_gaps: bool) -> CallTree {
    // (depth, position_adjustment, thread_id, thread_depth, node_idx)
    let mut node_map: HashMap<String, (usize, u128, NonZero<u64>, usize, NodeIndex)> = HashMap::new();
    let mut graph = StableDiGraph::new();
    let mut max_thread_depth = 1;
    let mut endpoint = 0;
    let mut relative_endpoint = 0;
    let mut startpoint = u128::MAX;
    let mut last_top_level_trace_end = 0;

    // iteration through events in received order
    for event in events {
        match &event {
            e @ TraceEvents::NewSpan {
                id,
                parent_id,
                weight,
                thread_id,
                created_at,
                ..
            } => {
                let weight = *weight;
                let mut node = CallNode {
                    id: id.clone(),
                    created_at: created_at.clone(),
                    depth: 1,
                    thread_depth: 1,
                    adjusted_timestamp: 0,
                    absolute_timestamp: weight,
                    total_duration: 0,
                    event: e.clone(),
                    color_bucket: 0.,
                };
                if weight < startpoint {
                    startpoint = weight;
                }
                if weight > endpoint {
                    endpoint = weight;
                }

                // Assign depth of this trace event vs its parent
                let mut depth = 1;
                if let Some(parent_id) = parent_id {
                    if let Some((parent_depth, _, _, _, _)) = node_map.get(parent_id) {
                        depth = parent_depth + 1;
                        node.depth = depth;
                    }
                }


                let mut our_thread_depth = 1;
                if let Some(parent_id) = parent_id {
                    if let Some((parent_depth, _, parent_thread_id, parent_thread_depth, _)) = node_map.get_mut(parent_id) {
                        // if this is not the same thread as our parent, increase depth by one
                        if thread_id != parent_thread_id {
                            *parent_thread_depth += 1;
                        }
                        our_thread_depth = *parent_thread_depth;
                        node.thread_depth = *parent_thread_depth;
                        max_thread_depth = max_thread_depth.max(*parent_thread_depth);
                    }
                }

                let mut position_subtracted_by_x_amount: u128 = 0;
                // If this is a child node, adjusted position is its absolute position - the parent's adjustment
                if let Some(parent_id) = parent_id {
                    if let Some((_, parent_adjustment_to_position, _, _, _)) = node_map.get(parent_id) {
                        position_subtracted_by_x_amount = *parent_adjustment_to_position;
                        node.adjusted_timestamp = weight - parent_adjustment_to_position;
                    }
                }

                // If this is a top-level node, adjust its position to the end of the last top-level node
                // If there is no completed top-level node, its adjustment is the start_time
                if parent_id.is_none() {
                    position_subtracted_by_x_amount = weight - last_top_level_trace_end;
                    node.adjusted_timestamp = last_top_level_trace_end;
                }

                // Store the relative endpoint of the trace as our extent if it is the max
                relative_endpoint = relative_endpoint.max(node.adjusted_timestamp);

                // Create the node and insert into the node_map
                let node_id = graph.add_node(node);
                node_map.insert(id.clone(), (depth, position_subtracted_by_x_amount, *thread_id, our_thread_depth, node_id));

                // Add an edge between parent and child
                if let Some(parent_id) = parent_id {
                    if let Some((_, _, _, _, parent)) = node_map.get(parent_id) {
                        graph.add_edge(*parent, node_id, ());
                    }
                }
            }
            TraceEvents::Enter(id) => {
            }
            TraceEvents::Exit(id, weight) => {
                if let Some(node) = graph.node_weight_mut(node_map[id].4) {
                    node.total_duration += weight - node.absolute_timestamp;
                    let endpoint_absolute = node.absolute_timestamp + node.total_duration;
                    let endpoint_adjusted = node.adjusted_timestamp + node.total_duration;

                    // If this is a top-level node, update the last top-level trace end
                    if node.depth == 1 {
                        last_top_level_trace_end = endpoint_adjusted;
                    }

                    relative_endpoint = endpoint_adjusted;

                    if endpoint_absolute > endpoint {
                        endpoint = endpoint_absolute;
                    }
                }
            }
            TraceEvents::Close(id, weight) => {
                // If the node is at the top of the stack
            }
            TraceEvents::Record => {}
            TraceEvents::Event => {}
        }
    }

    let mut vec_keys = vec![];
    for idx in graph.node_indices() {
        if let Some(call_node) = graph.node_weight(idx) {
            match &call_node.event {
                TraceEvents::NewSpan { name, location, line, thread_id, parent_id, .. } => {
                    vec_keys.push((idx, format!("{}{}", location, name)));
                }
                _ => {}
            }
        }
    }
    vec_keys.sort_by_key(|(idx, key)| key.clone());
    for (i, v ) in vec_keys.iter().enumerate() {
        graph.node_weight_mut(v.0).map(|node| {
            node.color_bucket = ((255. * i as f32) / vec_keys.len() as f32).floor();
        });
    }


    let now = Instant::now();
    // Set durations of anything incomplete to the current max time
    graph.node_weights_mut().for_each(|n| {
        if n.total_duration == 0 {
            n.total_duration = (n.created_at - now).as_nanos();
        }
    });
    CallTree {
        max_thread_depth,
        relative_endpoint,
        startpoint,
        endpoint,
        graph
    }
}

fn scale_to_target(v: u128, max_value: u128, target_max: f32) -> f32 {
    let scale_factor = target_max / max_value as f32;
    v as f32 * scale_factor
}

fn unscale_from_target(v: f32, max_value: u128, target_max: f32) -> u128 {
    let unscale_factor = max_value as f32 / target_max as f32;
    (v * unscale_factor) as u128
}


#[derive(Resource)]
struct SpanToTextMapping {
    spans: HashMap<String, Entity>,
    identity: HashMap<String, Entity>,
}


fn calculate_step_size(left: u128, right: u128, steps: u128) -> u128 {
    let interval_count = steps - 1;
    let raw_step_size = (right - left) / interval_count;
    let magnitude = 10_i64.pow(raw_step_size.to_string().len() as u32 - 1);
    let step_size = ((raw_step_size as f64 / magnitude as f64).round() * magnitude as f64);
    step_size as u128
}

#[derive(Resource, Default)]
struct TracesCallTree {
    inner: CallTree
}

fn maintain_call_tree(
    mut traces: ResMut<ChidoriTraceEvents>,
    mut call_tree: ResMut<TracesCallTree>,
) {
    call_tree.inner = build_call_tree(traces.inner.clone(), false);
}

fn update_positions(
    mut commands: Commands,
    mut gizmos: Gizmos,
    asset_server: Res<AssetServer>,
    mut query: Query<(&mut InstanceMaterialData,)>,
    mut span_to_text_mapping: ResMut<SpanToTextMapping>,
    mut q_text_elements: Query<(&Text, &mut Text2dBounds, &mut Transform), With<Text>>,
    mut q_span_identities: Query<(&IdentifiedSpan, &mut Collider, &mut Transform), (With<IdentifiedSpan>, Without<Text>)>,
    trace_camera_query: Query<(&Camera, &Projection, &GlobalTransform), With<TraceCameraTraces>>,
    text_camera_query: Query<(&Camera, &GlobalTransform), With<TraceCameraTextAtlas>>,
    mut trace_space: ResMut<TraceSpaceViewport>,
    mut call_tree: ResMut<TracesCallTree>,
    windows: Query<&Window>,
) {
    let scale_factor = windows.single().scale_factor();
    let span_height = SPAN_HEIGHT * scale_factor;
    let font = asset_server.load("fonts/CommitMono-1.143/CommitMono-400-Regular.otf");
    let text_style = TextStyle {
        font,
        font_size: 14.0,
        color: Color::WHITE,
    };

    let (trace_camera, trace_camera_projection, trace_camera_transform) = trace_camera_query.single();
    let (text_camera, text_camera_transform) = text_camera_query.single();

    let projection = match trace_camera_projection {
        Projection::Perspective(_) => {unreachable!("This should be orthographic")}
        Projection::Orthographic(o) => {o}
    };
    let camera_position = trace_camera_transform.translation();
    let viewport_width = projection.area.width();
    let viewport_height = projection.area.height();
    // left hand trace space coordinate
    let left = camera_position.x - viewport_width / 2.0;
    let right = camera_position.x + viewport_width / 2.0;

    let trace_to_text = |point: Vec3| -> Vec3 {
        let pos = trace_camera.world_to_ndc(trace_camera_transform, point).unwrap_or(Vec3::ZERO);
        text_camera.ndc_to_world(text_camera_transform, pos).unwrap_or(Vec3::ZERO)
    };

    let CallTree {
        max_thread_depth,
        relative_endpoint,
        startpoint: startpoint_value,
        endpoint: endpoint_value,
        graph: call_tree
    } = &call_tree.inner;

    if relative_endpoint < startpoint_value {
        return;
    }

    // Render the increment markings
    if endpoint_value > startpoint_value {
        // Shift into positive values
        let left_time_space = unscale_from_target(left + CAMERA_SPACE_WIDTH / 2.0, endpoint_value - startpoint_value, CAMERA_SPACE_WIDTH);
        let right_time_space = unscale_from_target(right + CAMERA_SPACE_WIDTH / 2.0, endpoint_value - startpoint_value, CAMERA_SPACE_WIDTH);
        let step_size = calculate_step_size(left_time_space, right_time_space, 10).max(1);
        let mut movement = 0;
        while movement <= right_time_space {
            let x = scale_to_target(movement, endpoint_value - startpoint_value, CAMERA_SPACE_WIDTH) - (CAMERA_SPACE_WIDTH / 2.0);
            let source_pos = Vec3::new(x, viewport_height/2.0, 0.0);
            let target_pos = Vec3::new(x, -100000.0, 0.0);
            gizmos.line(
                trace_to_text(source_pos),
                trace_to_text(target_pos),
                Color::Rgba {
                    red: 1.0,
                    green: 1.0,
                    blue: 1.0,
                    alpha: 0.05,
                },
            );
            movement += step_size;
        }
    }

    for (mut data,) in query.iter_mut() {
        let mut instances = data.0.iter_mut().collect::<Vec<_>>();
        let mut idx = 0;
        let root_nodes: Vec<NodeIndex> = call_tree.node_indices()
            .filter(|&node_idx| call_tree.edges_directed(node_idx, petgraph::Direction::Incoming).count() == 0)
            .collect();

        let mut max_vertical_extent : f32 = 0.0;
        for root_node in root_nodes {
            petgraph::visit::Dfs::new(&call_tree, root_node)
                .iter(&call_tree)
                .for_each(|node_idx| {
                    let node = call_tree.node_weight(node_idx).unwrap();
                    idx += 1;

                    // Filter rendering to only the currently viewed thread depth?
                    // Change the alpha transparency of increasing thread depth


                    // Scaled to 1000.0 unit width, offset to move from centered to left aligned
                    // let config_space_pos_x = scale_to_target(node.absolute_timestamp - startpoint_value, endpoint_value - startpoint_value, CAMERA_SPACE_WIDTH) - (CAMERA_SPACE_WIDTH / 2.0);
                    let config_space_pos_x = scale_to_target(node.adjusted_timestamp - 0, relative_endpoint - startpoint_value, CAMERA_SPACE_WIDTH) - (CAMERA_SPACE_WIDTH / 2.0);
                    let config_space_width = scale_to_target(node.total_duration, relative_endpoint - startpoint_value, CAMERA_SPACE_WIDTH);
                    let screen_space_pos_y = ((node.depth as f32) * -1.0 * span_height + span_height / 2.0) * node.thread_depth as f32;
                    max_vertical_extent = max_vertical_extent.max(screen_space_pos_y.abs() + span_height / 2.0);
                    instances[idx].color = color_for_bucket(node.color_bucket, (max_thread_depth - node.thread_depth) as f32 / *max_thread_depth as f32).as_rgba_f32();
                    instances[idx].width = config_space_width;
                    instances[idx].position.x = config_space_pos_x + (config_space_width / 2.0);
                    instances[idx].position.y = screen_space_pos_y;
                    instances[idx].vertical_scale = span_height;

                    instances[idx].border_color = Color::Rgba {
                        red: 0.0,
                        green: 0.0,
                        blue: 0.0,
                        alpha: 1.0,      // Fully opaque
                    }.as_rgba_f32();
                    // Hovered state
                    if let Some(&entity) = span_to_text_mapping.identity.get(&node.id) {
                        if let Ok((span, mut collider, mut transform)) = q_span_identities.get_mut(entity) {
                            if span.is_hovered {
                                instances[idx].border_color = Color::Rgba {
                                    red: 1.0,
                                    green: 1.0,
                                    blue: 1.0,
                                    alpha: 1.0,      // Fully opaque
                                }.as_rgba_f32();
                            }
                        }
                    }

                    let text_space_top_left_bound = trace_to_text(vec3(config_space_pos_x, screen_space_pos_y + span_height / 2.0, 0.0));
                    let text_space_bottom_right_bound = trace_to_text(vec3(config_space_pos_x + config_space_width, screen_space_pos_y - span_height / 2.0, 0.0));
                    let text_space_width = text_space_bottom_right_bound.x - text_space_top_left_bound.x;
                    let text_space_height = text_space_top_left_bound.y - text_space_bottom_right_bound.y;

                    let collision_pos = trace_to_text(vec3(config_space_pos_x + config_space_width / 2.0, screen_space_pos_y, 0.0));
                    let collision_width = text_space_width;
                    if HANDLE_COLLISIONS {
                        // // Update or Create collision instances
                        if let Some(&entity) = span_to_text_mapping.identity.get(&node.id) {
                            if let Ok((_, mut collider, mut transform)) = q_span_identities.get_mut(entity) {
                                transform.translation = collision_pos;
                                commands.entity(entity).remove::<Collider>();
                                commands.entity(entity).insert(Collider::cuboid(collision_width/2.0, text_space_height/2.0));
                            }
                        } else {
                            let identity = commands.spawn((
                                IdentifiedSpan {
                                    node_idx: node_idx,
                                    id: node.id.clone(),
                                    is_hovered: false,
                                },
                                TransformBundle::from_transform(Transform::from_translation(collision_pos)),
                                Collider::cuboid(collision_width/2.0, text_space_height/2.0),
                                Sensor,
                                OnTraceScreen
                            )).id();
                            span_to_text_mapping.identity.insert(node.id.clone(), identity);
                        }
                    }

                    // Adjust text position to fit within the bounds of the trace space and viewport
                    let mut text_pos_x = config_space_pos_x;
                    if text_pos_x < left && text_pos_x <= right {
                        text_pos_x = left;
                    }
                    // Convert to text space
                    let text_pos = trace_to_text(vec3(text_pos_x, screen_space_pos_y - (span_height / 4.0), 0.0));

                    // Get target width of the text area
                    let text_area_width = text_space_bottom_right_bound.x - text_pos.x;

                    // Update or Create text instances
                    if RENDER_TEXT {
                        if let Some(&entity) = span_to_text_mapping.spans.get(&node.id) {
                            if let Ok(mut text_bundle) = q_text_elements.get_mut(entity) {
                                text_bundle.2.translation = text_pos;
                                text_bundle.1.size = Vec2::new(text_area_width, if text_area_width < 5.0 { 0.0 } else { 1.0 });
                            }
                        } else {
                            let text = if let TraceEvents::NewSpan {name, location, line, ..} = &node.event {
                                format!("{}: {} ({})", name, location, line)
                            } else {
                                "???".to_string()
                            };
                            let style = text_style.clone();
                            let entity = commands.spawn((
                                Text2dBundle {
                                    text: Text {
                                        sections: vec![TextSection::new(text, style)],
                                        justify: JustifyText::Left,
                                        linebreak_behavior: BreakLineOn::AnyCharacter,
                                        ..default()
                                    },
                                    text_anchor: Anchor::BottomLeft,
                                    transform: Transform::from_translation(text_pos),
                                    text_2d_bounds: Text2dBounds {
                                        size: Vec2::new(text_area_width,  if text_area_width < 5.0 { 0.0 } else { 1.0 }),
                                    },
                                    ..default()
                                },
                                IdentifiedSpan {
                                    node_idx: node_idx,
                                    id: node.id.clone(),
                                    is_hovered: false,
                                },
                                RenderLayers::layer(4),
                                OnTraceScreen
                            )).id();
                            span_to_text_mapping.spans.insert(node.id.clone(), entity);
                        }
                    }

                });
        }
        trace_space.max_vertical_extent = if max_vertical_extent < 1.0 { viewport_height } else { max_vertical_extent };
    }
}


fn set_camera_viewports(
    windows: Query<&Window>,
    mut tree: ResMut<EguiTree>,
    mut trace_space: ResMut<TraceSpaceViewport>,
    mut resize_events: EventReader<WindowResized>,
    mut main_camera: Query<(&mut Camera, &mut Projection), (With<TraceCameraTraces>, Without<TraceCameraMinimap>, Without<TraceCameraTextAtlas>)>,
    mut text_camera: Query<&mut Camera, (With<TraceCameraTextAtlas>, Without<TraceCameraMinimap>, Without<TraceCameraTraces>)>,
    mut minimap_camera: Query<&mut Camera, (With<TraceCameraMinimap>, Without<TraceCameraTraces>, Without<TraceCameraTextAtlas>)>,
) {
    let window = windows.single();
    let scale_factor = window.scale_factor();
    let minimap_offset = MINIMAP_OFFSET * scale_factor as u32;
    let minimap_height = (MINIMAP_HEIGHT as f32 * scale_factor) as u32;
    let minimap_height_and_offset = MINIMAP_HEIGHT_AND_OFFSET * scale_factor as u32;
    let (mut main_camera , mut projection) = main_camera.single_mut();
    let mut text_camera = text_camera.single_mut();
    let mut minimap_camera = minimap_camera.single_mut();

    tree.tree.tiles.iter().for_each(|(_, tile)| {
        match tile {
            Tile::Pane(p) => {
                if &p.nr == &"Traces" {
                    if let Some(r) = p.rect {
                        main_camera.viewport = Some(Viewport {
                            physical_position: UVec2::new(r.min.x as u32, r.min.y as u32 + minimap_height_and_offset),
                            physical_size: UVec2::new(
                                r.width() as u32,
                                r.height() as u32 - minimap_height_and_offset,
                            ),
                            ..default()
                        });
                        text_camera.viewport = main_camera.viewport.clone();
                        minimap_camera.viewport = Some(Viewport {
                            physical_position: UVec2::new(r.min.x as u32, r.min.y as u32 + minimap_offset),
                            physical_size: UVec2::new(
                                r.width() as u32,
                                minimap_height,
                            ),
                            ..default()
                        });
                    }
                }
            }
            Tile::Container(_) => {}
        }
    });


    // We need to dynamically resize the camera's viewports whenever the window size changes
    // so then each camera always takes up half the screen.
    // A resize_event is sent when the window is first created, allowing us to reuse this system for initial setup.
    for resize_event in resize_events.read() {
        trace_space.vertical_scale = (window.resolution.physical_height() - minimap_height_and_offset) as f32;

        main_camera.viewport = Some(Viewport {
            physical_position: UVec2::new(0, minimap_height_and_offset),
            physical_size: UVec2::new(
                window.resolution.physical_width(),
                window.resolution.physical_height() - minimap_height_and_offset,
            ),
            ..default()
        });
        text_camera.viewport = main_camera.viewport.clone();
        minimap_camera.viewport = Some(Viewport {
            physical_position: UVec2::new(0, minimap_offset),
            physical_size: UVec2::new(
                window.resolution.physical_width(),
                minimap_height,
            ),
            ..default()
        });

    }
}



fn trace_setup(
    windows: Query<&Window>,
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    mut config_store: ResMut<GizmoConfigStore>,
) {
    let window = windows.single();
    let scale_factor = window.scale_factor();
    let (config, _) = config_store.config_mut::<DefaultGizmoConfigGroup>();
    config.line_width = 1.0;
    config.render_layers = RenderLayers::layer(4);

    let minimap_offset = MINIMAP_OFFSET * scale_factor as u32;
    let minimap_height = (MINIMAP_HEIGHT as f32 * scale_factor) as u32;
    let minimap_height_and_offset = MINIMAP_HEIGHT_AND_OFFSET * scale_factor as u32;

    commands.spawn((
        meshes.add(Mesh::from(Rectangle::new(1.0, 1.0))),
        SpatialBundle::INHERITED_IDENTITY,
        InstanceMaterialData(
            (1..=100)
                .flat_map(|x| 1..=100)
                .map(|_| InstanceData {
                    position: Vec3::new(-100000.0, -10000.0, -2.0),
                    width: 300.0,
                    vertical_scale: 300.0,
                    scale: 1.0,
                    border_color: Color::hsla(360., 1.0, 0.5, 1.0).as_rgba_f32(),
                    color: Color::hsla(360., 1.0, 0.5, 1.0).as_rgba_f32(),
                })
                .collect(),

        ),
        RenderLayers::layer(3),
        // NOTE: Frustum culling is done based on the Aabb of the Mesh and the GlobalTransform.
        // As the cube is at the origin, if its Aabb moves outside the view frustum, all the
        // instanced cubes will be culled.
        // The InstanceMaterialData contains the 'GlobalTransform' information for this custom
        // instancing, and that is not taken into account with the built-in frustum culling.
        // We must disable the built-in frustum culling by adding the `NoFrustumCulling` marker
        // component to avoid incorrect culling.
        NoFrustumCulling,
        OnTraceScreen,
    ));

    // Main trace view camera
    commands.spawn((
        Camera3dBundle {
            camera: Camera {
                order: 2,
                clear_color: ClearColorConfig::Custom(Color::rgba(0.1, 0.1, 0.1, 1.0)),
                viewport: Some(Viewport {
                    physical_position: UVec2::new(0, minimap_height_and_offset),
                    physical_size: UVec2::new(
                        window.resolution.physical_width(),
                        window.resolution.physical_height() - minimap_height_and_offset,
                    ),
                    ..default()
                }),
                ..default()
            },
            transform: Transform::from_xyz(0.0, 0.0, 1.0)
                .looking_at(Vec3::ZERO, Vec3::Y),
            projection: OrthographicProjection {
                scale: 1.0,
                scaling_mode: ScalingMode::Fixed {
                    width: CAMERA_SPACE_WIDTH,
                    height: (window.resolution.physical_height() - minimap_height_and_offset) as f32,
                },
                ..default()
            }.into(),
            ..default()
        },
        TraceCameraTraces,
        OnTraceScreen,
        RenderLayers::layer(3)
    ));

    // Text rendering camera
    commands.spawn((
        Camera2dBundle {
            camera: Camera {
                order: 4,
                viewport: Some(Viewport {
                    physical_position: UVec2::new(0, minimap_height_and_offset),
                    physical_size: UVec2::new(
                        window.resolution.physical_width(),
                        window.resolution.physical_height() - minimap_height_and_offset,
                    ),
                    ..default()
                }),
                ..default()
            },
            transform: Transform::from_xyz(0.0, 0.0, 0.0).looking_at(Vec3::ZERO, Vec3::Y),
            ..default()
        },
        OnTraceScreen,
        TraceCameraTextAtlas,
        RenderLayers::layer(4)
    ));

    // Minimap camera
    commands.spawn((
        Camera3dBundle {
            camera: Camera {
                order: 3,
                viewport: Some(Viewport {
                    physical_position: UVec2::new(0, minimap_offset),
                    physical_size: UVec2::new(
                        window.resolution.physical_width(),
                        minimap_height,
                    ),
                    ..default()
                }),
                ..default()
            },
            transform: Transform::from_xyz(0.0, 0.0, 1.0)
                .looking_at(Vec3::ZERO, Vec3::Y)
                .with_translation(Vec3::new(0.0, -(minimap_height as f32), 0.0)),
            projection: OrthographicProjection {
                scale: 1.0,
                scaling_mode: ScalingMode::Fixed {
                    width: CAMERA_SPACE_WIDTH,
                    height: (window.resolution.physical_height() - minimap_height) as f32,
                },
                ..default()
            }.into(),
            ..default()
        },
        TraceCameraMinimap,
        OnTraceScreen,
        RenderLayers::from_layers(&[3, 5])
    ));

    commands.spawn((
        PbrBundle {
            mesh: meshes.add(Mesh::from(Rectangle::new(1.0, 1.0))).into(),
            material: materials.add(Color::hsla(3.0, 1.0, 1.0, 0.5)),
            transform: Transform::from_xyz(0.0, -50.0, -1.0),
            ..default()
        },
        RenderLayers::layer(5),
        MinimapTraceViewport,
        NoFrustumCulling,
        OnTraceScreen,
    ));

    // Minimap viewport indicator
    // commands.spawn((
    //     Camera2dBundle {
    //         camera: Camera {
    //             order: 5,
    //             ..default()
    //         },
    //         transform: Transform::from_xyz(0.0, 0.0, 1.0).looking_at(Vec3::ZERO, Vec3::Y),
    //         ..default()
    //     },
    //     OnTraceScreen,
    //     TraceCameraMinimapDraw,
    //     RenderLayers::layer(5)
    // ));


    commands.spawn((CursorWorldCoords(Vec2::ZERO), OnTraceScreen));

    commands.insert_resource(SpanToTextMapping {
        spans: Default::default(),
        identity: Default::default(),
    });

    commands.insert_resource(TraceSpaceViewport {
        x: 0.0,
        y: 0.0,
        horizontal_scale: CAMERA_SPACE_WIDTH,
        vertical_scale: (window.resolution.physical_height() - minimap_height_and_offset) as f32,
        max_vertical_extent: SPAN_HEIGHT * scale_factor,
    });
    
}

#[derive(Component)]
struct TraceCameraTextAtlas;

#[derive(Component)]
struct TraceCameraTraces;

#[derive(Component)]
struct TraceCameraMinimap;

#[derive(Component)]
struct OnTraceScreen;

pub fn trace_plugin(app: &mut App) {
    app
        .init_resource::<TracesCallTree>()
        .add_plugins((CustomMaterialPlugin, ))
        .add_systems(OnEnter(crate::GameState::Traces), (trace_setup,))
        .add_systems(OnExit(crate::GameState::Traces), despawn_screen::<OnTraceScreen>)
        .add_systems(Update, (
            maintain_call_tree,
            update_trace_space_to_minimap_camera_configuration,
            update_trace_space_to_camera_configuration,
            update_positions,
            mouse_scroll_events,
            my_cursor_system,
            mouse_over_system,
            mouse_pan,
            touchpad_gestures,
            set_camera_viewports
        ).run_if(in_state(crate::GameState::Traces)));
}