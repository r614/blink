//! Import of a [`MapLayout`] (a map that is not one of the original's, e.g. the Playground) into
//! the same [`DishonoredMap`] a cooked level gives. The layout's packages are loaded only to
//! resolve object paths: none of their actors is placed. Brushes become world-space static
//! meshes with an original material; original static meshes, placed actors (doors, usable
//! objects, climbable chains, pickups) and lights are copied to where the layout puts them.
//! research/dishonored/PLAYGROUND.md.
//!
//! A child of `level` so it drives the level importer's own mesh, material and component code.

use super::*;
use dishonored_ir::layout::{Brush, LayoutLight, LightType, MapLayout, Shape, v3};

/// Import a layout. `opts.load_textures` false gives the geometry only (simulation).
pub fn import_layout(content: &Content, layout: &MapLayout, opts: &ImportOptions) -> Result<(DishonoredMap, ImportStats)> {
    let t0 = std::time::Instant::now();
    let mut pkgs = Vec::new();
    for p in &layout.packages {
        pkgs.push(content.package(p).with_context(|| format!("loading {p}"))?);
    }
    let layout_slots = pkgs.len();
    for p in LIBRARY_PACKAGES {
        if content.has_package(p) {
            pkgs.push(content.package(p).with_context(|| format!("loading {p}"))?);
        }
    }
    let mut index = HashMap::new();
    for (slot, pkg) in pkgs.iter().enumerate() {
        for i in 0..pkg.exports.len() {
            if RESOURCE_CLASSES.contains(&pkg.export_class(i).as_str()) {
                index.entry(pkg.obj_path(ObjRef::Export(i)).to_ascii_lowercase()).or_insert((slot, i));
            }
        }
    }
    let mut imp = Importer {
        content,
        opts,
        pkgs,
        index,
        map: DishonoredMap { name: layout.name.clone(), ..Default::default() },
        mesh_ids: HashMap::new(),
        mat_ids: HashMap::new(),
        tex_ids: HashMap::new(),
        pending_lightmaps: Vec::new(),
        merged_ids: HashMap::new(),
        stats: ImportStats::default(),
        ps_ids: HashMap::new(),
        class_defaults: HashMap::new(),
        skel_meshes: HashMap::new(),
        resolver: crate::collision_audit::PropResolver::new(content),
        tweaks: None,
    };
    imp.stats.timings.push(("packages", t0.elapsed().as_secs_f32()));
    let t = std::time::Instant::now();
    let slot_of = |imp: &Importer, name: &str| imp.pkgs[..layout_slots].iter().position(|p| p.name.eq_ignore_ascii_case(name));
    if let Some(env) = &layout.environment {
        match slot_of(&imp, env) {
            Some(s) => imp.import_environment(s),
            None => imp.stats.errors.push(format!("environment package {env} is not in the layout's packages")),
        }
    }
    let default_material = imp.material_by_path(&layout.default_material);
    for (i, b) in layout.brushes.iter().enumerate() {
        let mat = match &b.material {
            Some(p) => imp.material_by_path(p),
            None => default_material,
        };
        if b.material.is_some() && mat.is_none() {
            imp.stats.errors.push(format!("brush {i}: material {} not found", b.material.as_deref().unwrap_or("")));
        }
        imp.map.meshes.push(brush_mesh(b, mat, &format!("{}:Brush_{i}", layout.name)));
        imp.map.instances.push(MeshInstance {
            mesh: imp.map.meshes.len() - 1,
            transform: Mat4::IDENTITY,
            materials: Vec::new(),
            hidden: false,
            collides: b.collide,
            source: format!("{}:Brush_{i}", layout.name),
            lightmap: None,
            mover: None,
            trace_mask: dishonored_ir::all_trace_types(),
        });
    }
    for (i, m) in layout.meshes.iter().enumerate() {
        let Some(k) = imp.index.get(&m.mesh.to_ascii_lowercase()).copied() else {
            imp.stats.errors.push(format!("mesh {} not found in the layout's packages", m.mesh));
            continue;
        };
        let Some(mesh) = imp.mesh(k) else { continue };
        let materials = m.materials.iter().map(|p| p.as_ref().and_then(|p| imp.material_by_path(p))).collect();
        let r = |d: f32| (d * 65536.0 / 360.0).round() as i32;
        let transform = Mat4::from_translation(v3(m.at)) * rotator_matrix(r(m.rotation.0), r(m.rotation.1), r(m.rotation.2)) * Mat4::from_scale(v3(m.scale));
        let has_collision = !imp.map.meshes[mesh].collision_triangles.is_empty() || imp.map.meshes[mesh].simple_collision.is_some();
        imp.map.instances.push(MeshInstance {
            mesh,
            transform,
            materials,
            hidden: false,
            collides: m.collide && has_collision,
            source: format!("{}:Mesh_{i} ({})", layout.name, m.mesh),
            lightmap: None,
            mover: None,
            trace_mask: dishonored_ir::all_trace_types(),
        });
        imp.stats.static_mesh_actors += 1;
    }
    // Doors and usable objects of each package, read once.
    let mut usable: HashMap<usize, Vec<(dishonored_ir::SkeletalProp, u32)>> = HashMap::new();
    for a in &layout.actors {
        let (pkg_name, actor_name) = a.source();
        let Some(slot) = slot_of(&imp, pkg_name) else {
            imp.stats.errors.push(format!("actor {}: package {pkg_name} is not in the layout's packages", a.from));
            continue;
        };
        let pkg = imp.pkgs[slot].clone();
        let Some(actor) = find_placed_actor(&pkg, actor_name) else {
            imp.stats.errors.push(format!("actor {} not found", a.from));
            continue;
        };
        let class = pkg.export_class(actor);
        let props = pkg.read_object(actor)?.properties;
        let placed = Mat4::from_translation(vec3_or(&props, "Location", Vec3::ZERO)) * rot_matrix(&props, "Rotation");
        let delta = a.delta(placed);
        // Static mesh components (pickups, movables, the static parts of usables).
        let comps: Vec<usize> = (0..pkg.exports.len())
            .filter(|&i| matches!(pkg.export_class(i).as_str(), "StaticMeshComponent" | "SplineMeshComponent") && ObjRef::from_index(pkg.exports[i].outer) == ObjRef::Export(actor))
            .collect();
        let before = imp.map.instances.len();
        if !comps.is_empty() {
            if let Err(e) = imp.import_owned_components(slot, actor, &comps) {
                imp.stats.errors.push(format!("{}: {e:#}", a.from));
            }
        }
        // Skeletal scenery (climbable chains) in its reference pose.
        if SKELETAL_PROP_CLASSES.contains(&class.as_str()) {
            if let Some(ObjRef::Export(ci)) = find(&props, "SkeletalMeshComponent").and_then(|p| p.as_object()).map(ObjRef::from_index) {
                if let Err(e) = imp.import_skeletal_component(slot, actor, ci, &class) {
                    imp.stats.errors.push(format!("{}: {e:#}", a.from));
                }
            }
        }
        let new_name = a.actor_name(&layout.name);
        for inst in &mut imp.map.instances[before..] {
            inst.transform = delta * inst.transform;
            inst.mover = None;
            inst.source = format!("{new_name} <- {}", inst.source);
        }
        // Doors and usable objects: their skeletal prop, re-placed and renamed.
        if crate::usable_props::is_usable_class(&class) {
            let list = usable.entry(slot).or_insert_with(|| crate::usable_props::read_props_with_channels(content, std::slice::from_ref(&pkg.name)));
            let full = format!("{}:{}", pkg.name, actor_name);
            match list.iter().find(|p| p.0.actor.eq_ignore_ascii_case(&full)) {
                Some((p, _)) => {
                    let mut p = p.clone();
                    p.actor = new_name.clone();
                    p.transform = delta * p.transform;
                    p.actor_to_world = delta * p.actor_to_world;
                    if let (Some(d), Some(s)) = (p.door.as_mut(), a.door_state) {
                        d.initial = s;
                    }
                    p.light_env = None;
                    imp.map.skeletal_props.push(p);
                }
                None => imp.stats.errors.push(format!("usable {} has no skeletal mesh", a.from)),
            }
        }
    }
    for l in &layout.lights {
        match layout_light(&imp, l, layout_slots) {
            Ok(Some(light)) => imp.map.lights.push(light),
            Ok(None) => {}
            Err(e) => imp.stats.errors.push(e),
        }
    }
    for s in &layout.starts {
        imp.map.spawn_candidates.push(SpawnCandidate { position: v3(s.at), yaw_deg: s.yaw, source: format!("{}:PlayerStart {}", layout.name, s.tag) });
    }
    if opts.particles && opts.load_textures {
        for p in &opts.extra_particles {
            imp.particle_by_path(p);
        }
    }
    imp.stats.timings.push(("layout", t.elapsed().as_secs_f32()));
    imp.stats.instances = imp.map.instances.len();
    imp.stats.meshes = imp.map.meshes.len();
    imp.stats.materials = imp.map.materials.len();
    imp.stats.textures = imp.map.textures.len();
    Ok((imp.map, imp.stats))
}

/// Export index of an actor placed in a package's persistent level, by name.
pub fn find_placed_actor(pkg: &Package, name: &str) -> Option<usize> {
    (0..pkg.exports.len()).find(|&i| pkg.obj_name(ObjRef::Export(i)).eq_ignore_ascii_case(name) && pkg.obj_name(ObjRef::from_index(pkg.exports[i].outer)) == "PersistentLevel")
}

impl Importer<'_> {
    fn material_by_path(&mut self, path: &str) -> Option<MaterialId> {
        if path.is_empty() {
            return None;
        }
        let k = self.index.get(&path.to_ascii_lowercase()).copied()?;
        self.material(k)
    }
}

fn layout_light(imp: &Importer, l: &LayoutLight, layout_slots: usize) -> std::result::Result<Option<Light>, String> {
    if let Some(from) = &l.from {
        let (pkg_name, actor_name) = from.split_once(':').ok_or_else(|| format!("light {from}: expected <package>:<actor>"))?;
        let pkg = imp.pkgs[..layout_slots].iter().find(|p| p.name.eq_ignore_ascii_case(pkg_name)).ok_or_else(|| format!("light {from}: package not in the layout"))?;
        let actor = find_placed_actor(pkg, actor_name).ok_or_else(|| format!("light {from} not found"))?;
        let class = pkg.export_class(actor);
        let mut light = read_light(pkg, actor, &class)?;
        if let Some(light) = light.as_mut() {
            light.source = format!("{} (copy)", light.source);
            if !matches!(light.kind, LightKind::Directional) && l.at != (0.0, 0.0, 0.0) {
                light.position = v3(l.at);
            }
        }
        return Ok(light);
    }
    let kind = match l.kind {
        LightType::Point => LightKind::Point,
        LightType::Spot => {
            let (inner, outer) = l.cone.unwrap_or((0.0, 44.0));
            LightKind::Spot { inner_cone_deg: inner, outer_cone_deg: outer }
        }
        LightType::Directional => LightKind::Directional,
    };
    Ok(Some(Light {
        kind,
        position: v3(l.at),
        direction: v3(l.direction).normalize_or(Vec3::NEG_Z),
        color: [l.color.0, l.color.1, l.color.2],
        brightness: l.brightness,
        radius: l.radius,
        source: format!("layout light at {:?}", l.at),
        falloff_exponent: l.falloff_exponent,
        dominant: l.shadows,
        casts_shadows: l.shadows,
        guid: [0; 4],
    }))
}

/// A brush as a world-space static mesh: one section, planar texture mapping in world units,
/// per-polygon collision.
pub fn brush_mesh(b: &Brush, material: Option<MaterialId>, source: &str) -> StaticMeshAsset {
    let m = b.frame();
    // Each polygon in local space, wound later from its outward normal.
    let mut polys: Vec<(Vec<Vec3>, Vec3)> = Vec::new();
    for (lo, hi) in b.local_boxes() {
        let c = |x: f32, y: f32, z: f32| Vec3::new(x, y, z);
        polys.push((vec![c(hi.x, lo.y, lo.z), c(hi.x, hi.y, lo.z), c(hi.x, hi.y, hi.z), c(hi.x, lo.y, hi.z)], Vec3::X));
        polys.push((vec![c(lo.x, lo.y, lo.z), c(lo.x, hi.y, lo.z), c(lo.x, hi.y, hi.z), c(lo.x, lo.y, hi.z)], Vec3::NEG_X));
        polys.push((vec![c(lo.x, hi.y, lo.z), c(hi.x, hi.y, lo.z), c(hi.x, hi.y, hi.z), c(lo.x, hi.y, hi.z)], Vec3::Y));
        polys.push((vec![c(lo.x, lo.y, lo.z), c(hi.x, lo.y, lo.z), c(hi.x, lo.y, hi.z), c(lo.x, lo.y, hi.z)], Vec3::NEG_Y));
        polys.push((vec![c(lo.x, lo.y, hi.z), c(hi.x, lo.y, hi.z), c(hi.x, hi.y, hi.z), c(lo.x, hi.y, hi.z)], Vec3::Z));
        polys.push((vec![c(lo.x, lo.y, lo.z), c(hi.x, lo.y, lo.z), c(hi.x, hi.y, lo.z), c(lo.x, hi.y, lo.z)], Vec3::NEG_Z));
    }
    if let Shape::Ramp { width, length, angle } = &b.shape {
        let (w, l) = (width / 2.0, *length);
        let h = l * angle.to_radians().tan();
        let c = |x: f32, y: f32, z: f32| Vec3::new(x, y, z);
        polys.push((vec![c(0.0, -w, 0.0), c(l, -w, 0.0), c(l, w, 0.0), c(0.0, w, 0.0)], Vec3::NEG_Z));
        polys.push((vec![c(0.0, -w, 0.0), c(l, -w, h), c(l, w, h), c(0.0, w, 0.0)], Vec3::new(-h, 0.0, l).normalize()));
        polys.push((vec![c(l, -w, 0.0), c(l, w, 0.0), c(l, w, h), c(l, -w, h)], Vec3::X));
        polys.push((vec![c(0.0, w, 0.0), c(l, w, 0.0), c(l, w, h)], Vec3::Y));
        polys.push((vec![c(0.0, -w, 0.0), c(l, -w, 0.0), c(l, -w, h)], Vec3::NEG_Y));
    }
    let mut positions = Vec::new();
    let mut normals = Vec::new();
    let mut tangents = Vec::new();
    let mut uv0 = Vec::new();
    let mut indices: Vec<u32> = Vec::new();
    let scale = b.uv_scale.max(1.0);
    for (poly, n_local) in polys {
        let n = m.transform_vector3(n_local).normalize();
        let world: Vec<Vec3> = poly.iter().map(|p| m.transform_point3(*p)).collect();
        // Planar mapping on the plane most facing the normal.
        let (u_axis, v_axis) = if n.z.abs() >= n.x.abs() && n.z.abs() >= n.y.abs() {
            (Vec3::X, Vec3::Y)
        } else if n.x.abs() >= n.y.abs() {
            (Vec3::Y, Vec3::NEG_Z)
        } else {
            (Vec3::X, Vec3::NEG_Z)
        };
        let base = positions.len() as u32;
        for p in &world {
            positions.push(*p);
            normals.push(n);
            tangents.push((u_axis - n * u_axis.dot(n)).normalize_or(Vec3::X));
            uv0.push(Vec2::new(p.dot(u_axis) / scale, p.dot(v_axis) / scale));
        }
        for k in 1..world.len() - 1 {
            let (a, bb, c) = (world[0], world[k], world[k + 1]);
            // Wound so (b - a) x (c - a) points out of the solid, as the importer's meshes.
            if (bb - a).cross(c - a).dot(n) >= 0.0 {
                indices.extend([base, base + k as u32, base + k as u32 + 1]);
            } else {
                indices.extend([base, base + k as u32 + 1, base + k as u32]);
            }
        }
    }
    let (lo, hi) = positions.iter().fold((Vec3::splat(f32::MAX), Vec3::splat(f32::MIN)), |(a, c), p| (a.min(*p), c.max(*p)));
    let tris = indices.len() / 3;
    StaticMeshAsset {
        source: source.to_string(),
        positions,
        normals,
        tangents,
        uv0,
        uv1: Vec::new(),
        uv_set1: Vec::new(),
        collision_triangles: if b.collide { indices.chunks_exact(3).map(|t| [t[0], t[1], t[2]]).collect() } else { Vec::new() },
        collision_materials: if b.collide { vec![0; tris] } else { Vec::new() },
        indices,
        sections: vec![MeshSection { first_index: 0, num_triangles: tris as u32, material, enable_collision: b.collide }],
        simple_line: false,
        simple_box: false,
        simple_collision: None,
        bounds_min: lo,
        bounds_max: hi,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use dishonored_ir::layout::Shape;

    fn outward(mesh: &StaticMeshAsset) -> bool {
        let c = mesh.positions.iter().copied().sum::<Vec3>() / mesh.positions.len() as f32;
        mesh.indices.chunks_exact(3).all(|t| {
            let (a, b, d) = (mesh.positions[t[0] as usize], mesh.positions[t[1] as usize], mesh.positions[t[2] as usize]);
            (b - a).cross(d - a).dot((a + b + d) / 3.0 - c) > 0.0
        })
    }

    #[test]
    fn brush_box_is_closed_and_wound_out() {
        let b = Brush { at: (100.0, 0.0, 0.0), yaw: 30.0, shape: Shape::Box((200.0, 100.0, 44.0)), material: None, collide: true, uv_scale: 256.0 };
        let m = brush_mesh(&b, None, "t");
        assert_eq!(m.indices.len(), 36);
        assert_eq!(m.collision_triangles.len(), 12);
        assert!((m.bounds_max.z - 44.0).abs() < 1e-4 && m.bounds_min.z.abs() < 1e-4);
        assert!(outward(&m));
    }

    #[test]
    fn ramp_rises_at_its_angle() {
        let b = Brush { at: (0.0, 0.0, 0.0), yaw: 0.0, shape: Shape::Ramp { width: 300.0, length: 400.0, angle: 45.0 }, material: None, collide: true, uv_scale: 256.0 };
        let m = brush_mesh(&b, None, "t");
        assert!((m.bounds_max.z - 400.0).abs() < 1e-2);
        assert!(outward(&m));
        // The slope's normal is 45 degrees from vertical.
        let slope = m.normals.iter().find(|n| n.z > 0.1 && n.z < 0.99).unwrap();
        assert!((slope.z - 45f32.to_radians().cos()).abs() < 1e-4);
    }
}
