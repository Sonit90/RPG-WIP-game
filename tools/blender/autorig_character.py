"""Rigs an unrigged humanoid model onto the Mixamo skeleton, without Mixamo.

    blender -b --factory-startup --python tools/blender/autorig_character.py --
        --mesh <model.fbx|.glb|.obj> --out assets/characters/<hero>.fbx
        [--prop right|left] [--head-scale 1.2] [--blend <file.blend>]
        [--animations assets/animations]

(one command; split here for reading)

--mesh        the model: one standing humanoid, arms down, facing the viewer (-Y in
              Blender), any scale. A Meshy export works as downloaded.
--out         the rigged character, the file mixamo_character.py takes as --character
--prop        a held prop (sword, staff) that is its own loose part of the mesh: bound
              rigidly to that hand instead of being auto-weighted, so it does not bend
--head-scale  scales the head about the neck before rigging (1.0 leaves it alone)
--blend       optional .blend of the rigged result, to inspect in Blender
--animations  the folder holding the Mixamo animation downloads; Idle.fbx is read from it

What it does, and why each step is there: docs/character-animation.md, section 10.

The output stands in for a Mixamo download *With Skin*: the same 33 `mixamorig:` bones, in
the animation downloads' own units, so mixamo_character.py retargets onto it unchanged.
mixamo_character.py copies each bone's *world* rotation from the animation, so a bone whose
rest direction differs from the animation rig's tilts that part of the body in every clip.
The torso is therefore laid out in the animation rig's exact rest orientation, the limbs follow
the mesh, and arm twist comes from the animation rig's idle pose (arms down, like the model).

Joint heights are measured on the model normalised to 1.8 m, feet on the origin. They suit an
adult humanoid in a relaxed A-pose; a very different build may need the constants below moved.
"""
import argparse, bpy, bmesh, mathutils, mathutils.kdtree, os, sys
from mathutils import Vector as V

parser = argparse.ArgumentParser(prog="autorig_character.py")
parser.add_argument("--mesh", required=True, help="the unrigged model: .fbx, .glb/.gltf or .obj")
parser.add_argument("--out", required=True, help="the rigged .fbx to write")
REPO = os.path.dirname(os.path.dirname(os.path.dirname(os.path.abspath(__file__))))
parser.add_argument("--animations", default=os.path.join(REPO, "assets", "animations"),
                    help="folder holding the animation downloads (default: assets/animations)")
parser.add_argument("--prop", choices=("right", "left"), help="hand holding a separate prop")
parser.add_argument("--head-scale", type=float, default=1.0, help="head size multiplier")
parser.add_argument("--blend", help="optional .blend to save alongside")
args = parser.parse_args(sys.argv[sys.argv.index("--") + 1:] if "--" in sys.argv else [])

P = "mixamorig:"
HEIGHT = 1.8                     # the model is measured at this height; the constants assume it
REFERENCE = os.path.join(args.animations, "Idle.fbx")

bpy.ops.wm.read_factory_settings(use_empty=True)


def imported(path):
    objects = set(bpy.data.objects)
    ext = os.path.splitext(path)[1].lower()
    if ext == ".fbx":
        bpy.ops.import_scene.fbx(filepath=path)
    elif ext in (".glb", ".gltf"):
        bpy.ops.import_scene.gltf(filepath=path)
    elif ext == ".obj":
        bpy.ops.wm.obj_import(filepath=path)
    else:
        sys.exit(f"autorig: cannot import {path}")
    return [o for o in bpy.data.objects if o not in objects]


def select_only(*objects):
    bpy.ops.object.select_all(action="DESELECT")
    for o in objects:
        o.select_set(True)
    bpy.context.view_layer.objects.active = objects[0]


# --- The model: one mesh, 1.8 m tall, feet on the origin --------------------------------
meshes = [o for o in imported(args.mesh) if o.type == "MESH"]
for o in list(bpy.data.objects):
    if o not in meshes:
        bpy.data.objects.remove(o)
select_only(*meshes)
if len(meshes) > 1:
    bpy.ops.object.join()
model = bpy.context.view_layer.objects.active
model.name = model.data.name = "Body"
model.parent = None
bpy.ops.object.transform_apply(location=True, rotation=True, scale=True)
corners = [V(c) for c in model.bound_box]
low = V([min(c[i] for c in corners) for i in range(3)])
high = V([max(c[i] for c in corners) for i in range(3)])
model.data.transform(mathutils.Matrix.Scale(HEIGHT / (high.z - low.z), 4)
                     @ mathutils.Matrix.Translation(-V(((low.x + high.x) / 2,
                                                        (low.y + high.y) / 2, low.z))))
co = [v.co.copy() for v in model.data.vertices]

# Loose parts. A held prop is the part that reaches a third of the height front to back.
bm = bmesh.new()
bm.from_mesh(model.data)
part_of = [-1] * len(bm.verts)
parts = []
for v in bm.verts:
    if part_of[v.index] >= 0:
        continue
    parts.append([])
    stack = [v]
    while stack:
        x = stack.pop()
        if part_of[x.index] >= 0:
            continue
        part_of[x.index] = len(parts) - 1
        parts[-1].append(x.index)
        stack.extend(e.other_vert(x) for e in x.link_edges)
bm.free()
prop = set()
if args.prop:
    for p in parts:
        ys = [co[i].y for i in p]
        if max(ys) - min(ys) > HEIGHT / 3:
            prop |= set(p)
    if not prop:
        sys.exit("autorig: --prop given, but no loose part reaches a third of the height")
print("PROP verts", len(prop), "of", len(co))

# Bigger head: everything near the head's axis above the chin, scaled about the neck and
# ramped in over 8 cm so the collar does not tear.
if args.head_scale != 1.0:
    top = [v for i, v in enumerate(co) if i not in prop and v.z > 1.65]
    centre = sum(top, V()) / len(top)
    pivot = V((centre.x, centre.y, 1.50))
    for i, v in enumerate(model.data.vertices):
        p = co[i]
        if i in prop or p.z < 1.50 or (p.xy - pivot.xy).length > 0.16:
            continue
        share = min(1.0, (p.z - 1.50) / 0.08)
        co[i] = v.co = pivot + (p - pivot) * (1 + (args.head_scale - 1) * share)

body = [co[i] for i in range(len(co)) if i not in prop]
FLOOR, TOP = min(v.z for v in body), max(v.z for v in body)


def slab(z, lo, hi, dz=0.02):
    """Centre of the body's cross-section at height z, between x = lo and x = hi."""
    s = [v for v in body if abs(v.z - z) < dz and lo <= v.x <= hi]
    if not s:
        sys.exit(f"autorig: nothing of the body at z={z:.2f} between x={lo:.2f} and {hi:.2f}")
    return sum(s, V()) / len(s)


def height(share):
    return FLOOR + share * (TOP - FLOOR)


# --- Joint positions, measured on the mesh ----------------------------------------------
# The model faces -Y, so its right side is -X. cx is the head's centre line.
cx = slab(1.70, -1, 1).x
J = {}
for name, share in (("Hips", .591), ("Spine", .635), ("Spine1", .688), ("Spine2", .747),
                    ("Neck", .814), ("Head", .862)):
    J[name] = V((cx, slab(height(share), cx - 0.14, cx + 0.14).y, height(share)))
J["HeadTop_End"] = V((cx, J["Head"].y, TOP))
J["HeadTop_End_tail"] = V((cx, J["Head"].y, TOP + 0.1))
# Arms: the upper arm under the pauldron, elbow, wrist and the middle of the hand, each the
# centre of the arm's own cross-section outside the torso.
for side, sign, lo, hi in (("Right", -1, -1.0, cx - 0.24), ("Left", 1, cx + 0.30, 1.0)):
    upper, elbow = slab(1.28, lo, hi), slab(1.17, lo, hi)
    wrist, hand = slab(0.98, lo, hi), slab(0.89, lo, hi)
    J[side + "Shoulder"] = V((cx + sign * 0.05, upper.y, 1.44))
    J[side + "Arm"] = V((upper.x - sign * 0.03, upper.y, 1.42))
    J[side + "ForeArm"] = elbow
    J[side + "Hand"] = wrist
    along = (hand - wrist).normalized()
    J[side + "HandIndex1"] = hand
    for k in (2, 3, 4):
        J[side + f"HandIndex{k}"] = hand + along * 0.025 * (k - 1)
    J[side + "HandIndex4_tail"] = hand + along * 0.09
# Legs: knee and ankle cross-sections, the toe at the front of each foot.
for side, sign, lo, hi in (("Right", -1, -1.0, cx - 0.02), ("Left", 1, cx + 0.02, 1.0)):
    knee, ankle = slab(0.55, lo, hi), slab(0.16, lo, hi)
    toe = min((v for v in body if v.z < 0.06 and lo <= v.x <= hi), key=lambda v: v.y)
    J[side + "UpLeg"] = V((cx + sign * 0.15, J["Hips"].y, height(.566)))
    J[side + "Leg"] = knee
    J[side + "Foot"] = ankle
    J[side + "ToeBase"] = V((ankle.x + (toe.x - ankle.x) * 0.6, ankle.y + (toe.y - ankle.y) * 0.6, 0.04))
    J[side + "Toe_End"] = J[side + "ToeBase_tail"] = V((toe.x, toe.y, 0.03))
    J[side + "Toe_End_tail"] = V((toe.x, toe.y - 0.05, 0.03))

# --- The skeleton: the animation rig itself, its joints moved onto the mesh -------------
new = imported(REFERENCE)
rig = next(o for o in new if o.type == "ARMATURE")
for o in new:
    if o is not rig:
        bpy.data.objects.remove(o)
reference = rig.matrix_world.copy()
turn = reference.to_3x3()
# Rest pose (torso orientation) and the first idle frame (arm twist), in world space.
rest = {b.name[len(P):]: (reference @ b.head_local, reference @ b.tail_local,
                          (turn @ b.matrix_local.to_3x3()).col[2].normalized())
        for b in rig.data.bones}
action = rig.animation_data.action
bpy.context.scene.frame_set(int(action.frame_range[0]))
bpy.context.view_layer.update()
idle_z = {pb.name[len(P):]: (turn @ pb.matrix.to_3x3()).col[2].normalized() for pb in rig.pose.bones}
# The animation downloads' size: the output is written at it (see the export below).
reference_hips = (rest["Hips"][0] - rest["LeftToeBase"][0]).z
rig.animation_data.action = None
for pb in rig.pose.bones:
    pb.matrix_basis = mathutils.Matrix.Identity(4)
bpy.data.actions.remove(action)

select_only(rig)
bpy.ops.object.transform_apply(location=True, rotation=True, scale=True)
rig.name = "Armature"
bpy.ops.object.mode_set(mode="EDIT")
bones = rig.data.edit_bones
leg_z = {b.name: b.z_axis.copy() for b in bones}
for b in bones:
    b.use_connect = False
for b in bones:
    b.head = J[b.name[len(P):]]
for b in bones:
    name = b.name[len(P):]
    if name + "_tail" in J:
        b.tail = J[name + "_tail"]
    elif name.endswith("Shoulder"):
        b.tail = J[name.replace("Shoulder", "Arm")]
    elif name.endswith("Hand"):
        b.tail = J[name + "Index1"]
    elif b.children:
        b.tail = b.children[0].head
    b.align_roll(idle_z[name] if any(k in name for k in ("Shoulder", "Arm", "Hand")) else leg_z[b.name])
# Torso: the animation rig's rest orientation exactly, scaled to this body, set front to back
# halfway between the mesh's hips and head centres.
k = (J["Head"].z - J["Hips"].z) / (rest["Head"][0].z - rest["Hips"][0].z)
base = J["Hips"].copy()
base.y += 0.5 * (J["Head"].y - (base.y + (rest["Head"][0].y - rest["Hips"][0].y) * k))
for name in ("Hips", "Spine", "Spine1", "Spine2", "Neck", "Head", "HeadTop_End"):
    b = bones[P + name]
    head, tail, z = rest[name]
    b.head = base + (head - rest["Hips"][0]) * k
    b.tail = base + (tail - rest["Hips"][0]) * k
    b.align_roll(z)
for b in bones:
    if b.parent and (b.parent.tail - b.head).length < 1e-4:
        b.use_connect = True
bpy.ops.object.mode_set(mode="OBJECT")

# --- Skin -------------------------------------------------------------------------------
select_only(rig, model)
bpy.ops.object.parent_set(type="ARMATURE_AUTO")
groups = model.vertex_groups
if prop:
    for g in groups:
        g.remove(list(prop))
    groups[P + args.prop.capitalize() + "Hand"].add(list(prop), 1.0, "REPLACE")

# Vertices heat weighting gave nothing: the nearest bone, fully.
segments = [(b.name, b.head_local, b.tail_local) for b in rig.data.bones
            if not b.name.endswith(("_End", "Index4"))]


def nearest_bone(p):
    def distance(seg):
        _, a, c = seg
        t = max(0.0, min(1.0, (p - a).dot(c - a) / max((c - a).length_squared, 1e-9)))
        return (a + (c - a) * t - p).length
    return min(segments, key=distance)[0]


for v in model.data.vertices:
    if sum(g.weight for g in v.groups) < 1e-3:
        groups[nearest_bone(co[v.index])].add([v.index], 1.0, "REPLACE")

# Small loose parts (straps, pads, pouches) take heat weights badly and fly off in motion:
# each vertex takes the blended weights of the nearest vertices of the big parts.
largest = max(len(p) for p in parts)
donors = [i for p in parts if len(p) >= 0.15 * largest for i in p if i not in prop]
tree = mathutils.kdtree.KDTree(len(donors))
for n, i in enumerate(donors):
    tree.insert(co[i], n)
tree.balance()
weights = [{g.group: g.weight for g in v.groups} for v in model.data.vertices]
for p in parts:
    if len(p) >= 0.15 * largest or p[0] in prop:
        continue
    for i in p:
        blend = {}
        for _, n, d in tree.find_n(co[i], 6):
            for g, wt in weights[donors[n]].items():
                blend[g] = blend.get(g, 0.0) + wt / max(d, 1e-4)
        total = sum(blend.values())
        for g in groups:
            g.remove([i])
        for g, wt in blend.items():
            groups[g].add([i], wt / total, "REPLACE")

# The importer refuses a fifth influence.
select_only(model)
bpy.ops.object.vertex_group_limit_total(group_select_mode="ALL", limit=4)
bpy.ops.object.vertex_group_normalize_all(group_select_mode="ALL", lock_active=False)

# --- Export -----------------------------------------------------------------------------
# mixamo_character.py scales the animations by the same factor as the character, so the
# character is written at the animation downloads' size: hips as high above the toes.
hips = (rig.data.bones[P + "Hips"].head_local - rig.data.bones[P + "LeftToeBase"].head_local).z
scale = reference_hips / hips
select_only(rig, model)
bpy.ops.export_scene.fbx(filepath=os.path.abspath(args.out), object_types={"ARMATURE", "MESH"},
                         global_scale=scale, apply_scale_options="FBX_SCALE_ALL",
                         add_leaf_bones=False, bake_anim=False, path_mode="COPY",
                         embed_textures=True)
if args.blend:
    bpy.ops.wm.save_as_mainfile(filepath=os.path.abspath(args.blend))
print("DONE", args.out, len(rig.data.bones), "bones")
