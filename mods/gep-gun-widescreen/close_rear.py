# Blender (4.x / 5.x) script of the "GEP gun widescreen" mod: closes the rear right corner of the GEP gun's
# first-person model (DeusExItems.GEPGun). Run by build.sh:
#   blender -b --python close_rear.py -- <GEPGun.gltf from deusex-modkit export> <frame 0 OBJ, --stored> <out.glb>
#
# The model was made for a 4:3 screen: its rear plate stops where the deck above it still flares out to the right,
# right at the old screen edge, so on wider screens the corner is a thin ledge with nothing under it. This adds the
# block under the ledge: the rear plate carried on to the ledge's outer corner (textured as the plate, mirrored
# across their shared edge, so it continues seamlessly), and the block's outer side and bottom (on a plain panel of
# the same texture). The two new vertices are left where they are in every shape key (as an artist's edit leaves
# them); `deusex-modkit import` binds them to the nearest original face, so they follow the gun in every frame.
import sys
import bpy
import bmesh
from mathutils import Vector

gltf, obj, out = sys.argv[sys.argv.index("--") + 1:][:3]

# vertex numbers of the original mesh (as `deusex-modkit export-obj` numbers them): the rear plate's inner top (2) and
# bottom (86) corners and one more of its corners (41), the ledge's inner (177) and outer (178) rear corners and its
# front tip (179)
P2, P41, P86, P177, P178, P179 = 2, 41, 86, 177, 178, 179

pos = []
for line in open(obj):
    if line.startswith("v "):
        x, y, z = map(float, line.split()[1:4])
        pos.append(Vector((x, -z, y)))  # glTF/OBJ y up -> Blender z up

bpy.ops.wm.read_factory_settings(use_empty=True)
bpy.ops.import_scene.gltf(filepath=gltf)
ob = next(o for o in bpy.data.objects if o.type == "MESH")
me = ob.data
bm = bmesh.new()
bm.from_mesh(me)
bm.verts.ensure_lookup_table()


def vert_at(p):
    v = min(bm.verts, key=lambda v: (v.co - p).length)
    assert (v.co - p).length < 1e-3, f"no vertex at {p} (is this the original GEPGun?)"
    return v


v2, v41, v86, v177, v178, v179 = (vert_at(pos[i]) for i in (P2, P41, P86, P177, P178, P179))
down = v86.co - v2.co  # the plate's edge from its top to its bottom
n1 = bm.verts.new(v178.co + down)  # under the ledge's outer corner
n2 = bm.verts.new(v179.co + down)  # under the ledge's tip
# new vertices keep their place in every shape key
for layer in bm.verts.layers.shape.values():
    n1[layer] = n1.co
    n2[layer] = n2.co

uv = bm.loops.layers.uv.active


def loop_uv(v, f):
    return next(l[uv].uv.copy() for l in f.loops if l.vert == v)


# the plate's texture mapping, from the face holding its corners 2, 86 and 41
plate = next(f for f in bm.faces if {v2, v86, v41} <= set(f.verts))
o, e1, e2 = v2.co, v86.co - v2.co, v41.co - v2.co
t0, t1, t2 = loop_uv(v2, plate), loop_uv(v86, plate), loop_uv(v41, plate)
edge_u = t0.x  # the edge 2-86 runs along constant u


def plate_uv(p):
    # p in the plate's plane as o + a e1 + b e2 (least squares), mapped, then mirrored across the edge 2-86
    d = p - o
    g11, g12, g22 = e1.dot(e1), e1.dot(e2), e2.dot(e2)
    r1, r2 = d.dot(e1), d.dot(e2)
    det = g11 * g22 - g12 * g12
    a, b = (r1 * g22 - r2 * g12) / det, (r2 * g11 - r1 * g12) / det
    t = t0 + a * (t1 - t0) + b * (t2 - t0)
    return Vector((2 * edge_u - t.x, t.y))


def panel(u, v):
    # a plain panel of the texture (in the game's 0-255 texel steps, from the top)
    return Vector((u / 256, 1 - v / 256))


centre = sum((v.co for v in (v2, v86, v177, v178, v179, n1, n2)), Vector()) / 7
faces = [
    ([v2, v177, v178, n1, v86], None),  # rear: the plate carried on under the ledge
    ([v178, v179, n2, n1], [panel(203, 250), panel(203, 150), panel(250, 150), panel(250, 250)]),  # outer side
    ([v86, n1, n2], [panel(210, 250), panel(250, 250), panel(250, 160)]),  # bottom
]
for verts, uvs in faces:
    # outward: away from the block's centre
    nrm = (verts[1].co - verts[0].co).cross(verts[2].co - verts[0].co)
    if nrm.dot(sum((v.co for v in verts), Vector()) / len(verts) - centre) < 0:
        verts = verts[::-1]
        uvs = uvs[::-1] if uvs else None
    f = bm.faces.new(verts)
    f.material_index = plate.material_index
    for k, l in enumerate(f.loops):
        l[uv].uv = uvs[k] if uvs else plate_uv(l.vert.co)

bm.to_mesh(me)
bm.free()
print(f"close_rear: {len(me.vertices)} vertices, {len(me.polygons)} faces")
bpy.ops.export_scene.gltf(filepath=out, export_format="GLB")
