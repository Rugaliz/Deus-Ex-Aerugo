//! `Polys` and `Model` objects: brush polygons and BSP geometry (spec: `docs/formats/model-format.md`).

use crate::payload;
use ue_package::{Error, Package, Reader, Result};

pub type Vec3 = [f32; 3];

fn vec3(r: &mut Reader) -> Result<Vec3> {
    Ok([r.f32()?, r.f32()?, r.f32()?])
}

fn count(r: &mut Reader, what: &str, elem_min: usize) -> Result<usize> {
    let n = r.compact_index()?;
    if n < 0 || (n as usize).saturating_mul(elem_min) > 1 << 28 {
        return Err(Error::Invalid(format!("{what} count {n}")));
    }
    Ok(n as usize)
}

/// A convex editor polygon.
#[derive(Debug, Clone, PartialEq)]
pub struct Poly {
    pub base: Vec3,
    pub normal: Vec3,
    pub texture_u: Vec3,
    pub texture_v: Vec3,
    pub vertices: Vec<Vec3>,
    pub poly_flags: u32,
    /// Raw object references (compact indices): the brush actor, the texture; the item name is a name index.
    pub actor: i32,
    pub texture: i32,
    pub item_name: i32,
    pub i_link: i32,
    pub i_brush_poly: i32,
    pub pan_u: i16,
    pub pan_v: i16,
}

fn poly(r: &mut Reader) -> Result<Poly> {
    let n = count(r, "polygon vertex", 12)?;
    let (base, normal, texture_u, texture_v) = (vec3(r)?, vec3(r)?, vec3(r)?, vec3(r)?);
    let vertices = (0..n).map(|_| vec3(r)).collect::<Result<Vec<_>>>()?;
    Ok(Poly {
        base,
        normal,
        texture_u,
        texture_v,
        vertices,
        poly_flags: r.u32()?,
        actor: r.compact_index()?,
        texture: r.compact_index()?,
        item_name: r.compact_index()?,
        i_link: r.compact_index()?,
        i_brush_poly: r.compact_index()?,
        pan_u: r.u16()? as i16,
        pan_v: r.u16()? as i16,
    })
}

/// A `Polys` object: the polygon list of a brush (or of the level's source geometry).
#[derive(Debug, Clone)]
pub struct Polys {
    pub polys: Vec<Poly>,
}

impl Polys {
    pub fn load(pkg: &Package, export: usize) -> Result<Polys> {
        let p = payload(pkg, export)?;
        let mut r = Reader::new(p.data, p.native_start);
        let num = r.i32()?;
        let _max = r.i32()?;
        if num < 0 || num as usize > 1 << 24 {
            return Err(Error::Invalid(format!("polygon count {num}")));
        }
        let polys = (0..num).map(|_| poly(&mut r)).collect::<Result<Vec<_>>>()?;
        if r.pos() != p.data.len() {
            return Err(Error::Invalid(format!("polys has {} trailing bytes", p.data.len() - r.pos())));
        }
        Ok(Polys { polys })
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct BspNode {
    /// Splitting plane: normal and distance.
    pub plane: [f32; 4],
    pub zone_mask: u64,
    pub node_flags: u8,
    pub i_vert_pool: i32,
    pub i_surf: i32,
    pub i_back: i32,
    pub i_front: i32,
    pub i_plane: i32,
    pub i_collision_bound: i32,
    pub i_render_bound: i32,
    pub i_zone: [u8; 2],
    pub num_vertices: u8,
    pub i_leaf: [i32; 2],
}

#[derive(Debug, Clone, PartialEq)]
pub struct BspSurf {
    /// Object reference (compact index) of the texture.
    pub texture: i32,
    pub poly_flags: u32,
    pub p_base: i32,
    pub v_normal: i32,
    pub v_texture_u: i32,
    pub v_texture_v: i32,
    pub i_light_map: i32,
    pub i_brush_poly: i32,
    pub pan_u: i16,
    pub pan_v: i16,
    pub actor: i32,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Vert {
    pub p_vertex: i32,
    pub i_side: i32,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Zone {
    /// Object reference of the zone's `ZoneInfo` actor.
    pub zone_actor: i32,
    pub connectivity: u64,
    pub visibility: u64,
}

#[derive(Debug, Clone, PartialEq)]
pub struct LightMapIndex {
    pub data_offset: i32,
    pub pan: Vec3,
    pub u_clamp: i32,
    pub v_clamp: i32,
    pub u_scale: f32,
    pub v_scale: f32,
    pub i_light_actors: i32,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Leaf {
    pub i_zone: i32,
    pub i_permeating: i32,
    pub i_volumetric: i32,
    pub visible_zones: u64,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Bounds {
    pub min: Vec3,
    pub max: Vec3,
    pub is_valid: bool,
}

#[derive(Debug, Clone, Default)]
pub struct Model {
    pub bounding_box: Bounds,
    /// Bounding sphere: centre and radius.
    pub bounding_sphere: [f32; 4],
    pub vectors: Vec<Vec3>,
    pub points: Vec<Vec3>,
    pub nodes: Vec<BspNode>,
    pub surfs: Vec<BspSurf>,
    pub verts: Vec<Vert>,
    pub num_shared_sides: i32,
    pub zones: Vec<Zone>,
    /// Object reference of the model's `Polys` (0 when it has none).
    pub polys: i32,
    pub light_map: Vec<LightMapIndex>,
    pub light_bits: Vec<u8>,
    pub bounds: Vec<Bounds>,
    pub leaf_hulls: Vec<i32>,
    pub leaves: Vec<Leaf>,
    /// Object references of the lights.
    pub lights: Vec<i32>,
    pub root_outside: bool,
    pub linked: bool,
}

fn bounds(r: &mut Reader) -> Result<Bounds> {
    Ok(Bounds { min: vec3(r)?, max: vec3(r)?, is_valid: r.u8()? != 0 })
}

fn array<T>(r: &mut Reader, what: &str, elem_min: usize, mut f: impl FnMut(&mut Reader) -> Result<T>) -> Result<Vec<T>> {
    let n = count(r, what, elem_min)?;
    (0..n).map(|_| f(r)).collect()
}

impl Model {
    pub fn load(pkg: &Package, export: usize) -> Result<Model> {
        if pkg.version < 62 {
            return Err(Error::Invalid("models older than package version 62 are not supported".into()));
        }
        let p = payload(pkg, export)?;
        let r = &mut Reader::new(p.data, p.native_start);
        // UPrimitive: bounding box, then bounding sphere
        let bounding_box = bounds(r)?;
        let bounding_sphere = [r.f32()?, r.f32()?, r.f32()?, r.f32()?];
        let vectors = array(r, "vector", 12, vec3)?;
        let points = array(r, "point", 12, vec3)?;
        let nodes = array(r, "node", 30, |r| {
            Ok(BspNode {
                plane: [r.f32()?, r.f32()?, r.f32()?, r.f32()?],
                zone_mask: r.u64()?,
                node_flags: r.u8()?,
                i_vert_pool: r.compact_index()?,
                i_surf: r.compact_index()?,
                i_back: r.compact_index()?,
                i_front: r.compact_index()?,
                i_plane: r.compact_index()?,
                i_collision_bound: r.compact_index()?,
                i_render_bound: r.compact_index()?,
                i_zone: [r.u8()?, r.u8()?],
                num_vertices: r.u8()?,
                i_leaf: [r.i32()?, r.i32()?],
            })
        })?;
        let surfs = array(r, "surface", 20, |r| {
            Ok(BspSurf {
                texture: r.compact_index()?,
                poly_flags: r.u32()?,
                p_base: r.compact_index()?,
                v_normal: r.compact_index()?,
                v_texture_u: r.compact_index()?,
                v_texture_v: r.compact_index()?,
                i_light_map: r.compact_index()?,
                i_brush_poly: r.compact_index()?,
                pan_u: r.u16()? as i16,
                pan_v: r.u16()? as i16,
                actor: r.compact_index()?,
            })
        })?;
        let verts = array(r, "vertex", 2, |r| Ok(Vert { p_vertex: r.compact_index()?, i_side: r.compact_index()? }))?;
        let num_shared_sides = r.i32()?;
        let num_zones = r.i32()?;
        if !(0..=64).contains(&num_zones) {
            return Err(Error::Invalid(format!("zone count {num_zones}")));
        }
        let zones = (0..num_zones)
            .map(|_| Ok(Zone { zone_actor: r.compact_index()?, connectivity: r.u64()?, visibility: r.u64()? }))
            .collect::<Result<Vec<_>>>()?;
        let polys = r.compact_index()?;
        let light_map = array(r, "light map index", 36, |r| {
            let data_offset = r.i32()?;
            let pan = vec3(r)?;
            let (u_clamp, v_clamp) = (r.compact_index()?, r.compact_index()?);
            Ok(LightMapIndex { data_offset, pan, u_clamp, v_clamp, u_scale: r.f32()?, v_scale: r.f32()?, i_light_actors: r.i32()? })
        })?;
        let n = count(r, "light bit", 1)?;
        let light_bits = r.bytes(n)?.to_vec();
        let bounds_list = array(r, "bounds", 25, bounds)?;
        let leaf_hulls = array(r, "leaf hull", 4, |r| r.i32())?;
        let leaves = array(r, "leaf", 11, |r| {
            Ok(Leaf { i_zone: r.compact_index()?, i_permeating: r.compact_index()?, i_volumetric: r.compact_index()?, visible_zones: r.u64()? })
        })?;
        let lights = array(r, "light", 1, |r| r.compact_index())?;
        let root_outside = r.i32()? != 0;
        let linked = r.i32()? != 0;
        if r.pos() != p.data.len() {
            return Err(Error::Invalid(format!("model has {} trailing bytes", p.data.len() as i64 - r.pos() as i64)));
        }
        Ok(Model {
            bounding_box,
            bounding_sphere,
            vectors,
            points,
            nodes,
            surfs,
            verts,
            num_shared_sides,
            zones,
            polys,
            light_map,
            light_bits,
            bounds: bounds_list,
            leaf_hulls,
            leaves,
            lights,
            root_outside,
            linked,
        })
    }
}

/// One drawable polygon of the world BSP: a node's vertices with the surface it belongs to.
#[derive(Debug, Clone)]
pub struct Face {
    pub node: usize,
    pub surf: usize,
    pub vertices: Vec<Vec3>,
}

impl Model {
    /// The convex polygons of every BSP node that has vertices, in node order.
    pub fn faces(&self) -> Vec<Face> {
        let mut out = Vec::new();
        for (ni, n) in self.nodes.iter().enumerate() {
            if n.num_vertices < 3 || n.i_surf < 0 {
                continue;
            }
            let pool = &self.verts[n.i_vert_pool as usize..][..n.num_vertices as usize];
            let vertices = pool.iter().map(|v| self.points[v.p_vertex as usize]).collect();
            out.push(Face { node: ni, surf: n.i_surf as usize, vertices });
        }
        out
    }
}
