"""Builds a character's .glb: a Mixamo-rigged model plus the game's animations.

    blender -b --factory-startup --python tools/blender/mixamo_character.py --
        --character assets/characters/firemage.fbx
        --out assets/firemage.glb [--blend assets/firemage.blend] [--height 1.8]
        [--animations assets/animations]

(one command; split here for reading)

--character  the character: a Mixamo download *With Skin* (any animation, it is discarded),
             kept as assets/characters/<hero>.fbx
--animations the folder holding the animation downloads named in CLIPS below;
             assets/animations in this repository unless given
--out        the .glb the game loads (assets/<hero file name>)
--blend      optional .blend to inspect the result in Blender
--height     how tall the character stands, metres

The whole workflow, and why each step is there: docs/character-animation.md.

Animations are *retargeted*, not copied: each one is played on the rig it was downloaded
with, and the character's bones follow it in world space before being baked. Mixamo's local
keys only mean the same thing on the rig they were made for, and an animation downloaded
while a different character was selected in Mixamo crosses the arms if copied as-is.
"""
import argparse, bpy, math, mathutils, os, sys

parser = argparse.ArgumentParser(prog="mixamo_character.py")
parser.add_argument("--character", required=True, help="Mixamo download of the character, With Skin")
# The shared animation downloads live beside the models, two directories up from this script.
REPO = os.path.dirname(os.path.dirname(os.path.dirname(os.path.abspath(__file__))))
parser.add_argument("--animations", default=os.path.join(REPO, "assets", "animations"),
                    help="folder holding the animation downloads (default: assets/animations)")
parser.add_argument("--out", required=True, help="the .glb to write")
parser.add_argument("--blend", help="optional .blend to save alongside")
parser.add_argument("--height", type=float, default=1.8, help="standing height, metres")
args = parser.parse_args(sys.argv[sys.argv.index("--") + 1:] if "--" in sys.argv else [])

D = os.path.join(args.animations, "")
OUT = args.out
BLEND = args.blend
BASE = args.character
# clip name -> (file, first frame, last frame, airborne). None takes the file's whole range.
# `airborne` stops the hips rising above standing height: the game lifts the character
# when it jumps, and a Mixamo jump lifting it too would double the height.
CLIPS = {
    "idle": ("Idle.fbx", None, None, False),
    "walk": ("Walking.fbx", None, None, False),
    "run": ("Fast Run.fbx", None, None, False),
    "jump": ("Jumping.fbx", 14, 25, True),       # end of the crouch to the top of the jump
    "fall": ("Jumping.fbx", 27, 27, True),       # one pose, held while falling
    "land": ("Land.fbx", 6, 12, False),          # from touchdown on
    "cast": ("Cast.fbx", None, None, False),
    "cast_heavy": ("CastHeavy.fbx", None, None, False),
    # Optional: the game falls back without them. A file that is not there is skipped.
    "walk_back": ("Walking Backwards.fbx", None, None, False),
    "strafe_left": ("Left Strafe.fbx", None, None, False),
    # Remove this line and the game mirrors strafe_left into the right-hand strafe.
    "strafe_right": ("Right Strafe.fbx", None, None, False),
    "death": ("Death.fbx", None, None, False),  # one-shot, holds its last frame lying down
}
# Seconds into a cast clip at which the spell leaves the hands. Exported as the clip's glTF
# extras; the game lines this moment up with the cast completing and plays the rest after.
RELEASE = {"cast": 0.65, "cast_heavy": 1.0}
HEIGHT = args.height                      # Mixamo often reads a 1-unit model as centimetres
ROOT = "mixamorig:Hips"

bpy.ops.wm.read_factory_settings(use_empty=True)
scene = bpy.context.scene
scene.render.fps = 30


def imported(path):
    objects, actions = set(bpy.data.objects), set(bpy.data.actions)
    bpy.ops.import_scene.fbx(filepath=path)
    return ([o for o in bpy.data.objects if o not in objects],
            [a for a in bpy.data.actions if a not in actions])


def depth(bone):
    return 0 if bone.parent is None else 1 + depth(bone.parent)


# --- The character --------------------------------------------------------------------
objs, base_actions = imported(BASE)
rig = next(o for o in objs if o.type == 'ARMATURE')
rig.name = "Armature"
rig.animation_data_create()
rig.animation_data.action = None
for a in base_actions:
    bpy.data.actions.remove(a)
for pb in rig.pose.bones:
    pb.rotation_mode = 'QUATERNION'
    pb.matrix_basis = mathutils.Matrix.Identity(4)

# Real size, turned to face -Z in glTF (engine forward), feet on the origin.
mesh = next(o for o in rig.children if o.type == 'MESH')
corners = lambda: [mesh.matrix_world @ mathutils.Vector(c) for c in mesh.bound_box]
zs = [c.z for c in corners()]
turn = mathutils.Matrix.Rotation(math.pi, 4, 'Z') @ mathutils.Matrix.Scale(HEIGHT / (max(zs) - min(zs)), 4)
rig.matrix_world = turn @ rig.matrix_world
bpy.context.view_layer.update()
hips = rig.matrix_world @ rig.data.bones[ROOT].head_local
floor = min(c.z for c in corners())
place = mathutils.Matrix.Translation((-hips.x, -hips.y, -floor))
rig.matrix_world = place @ rig.matrix_world
bpy.context.view_layer.update()

bones = sorted(rig.pose.bones, key=lambda pb: depth(pb.bone))

# --- Each animation, retargeted and baked ----------------------------------------------
for name, (file, first, last, airborne) in CLIPS.items():
    if not os.path.exists(D + file):
        print("SKIP", name, "-", file, "is not in", D)
        continue
    objs, actions = imported(D + file)
    source = next(o for o in objs if o.type == 'ARMATURE')
    source_action = source.animation_data.action
    start, end = (int(round(f)) for f in source_action.frame_range)
    first = start if first is None else first
    last = end if last is None else last

    # Same turn and scale as the character. Then line the two up: hips over hips, and the
    # animation's ground on the character's rest-pose floor. A rig's rest pose is not always
    # standing height, so its hips are no guide to where the ground is; the lowest the toes
    # reach anywhere in the file is, since a Mixamo clip starts or ends on the ground.
    source.matrix_world = place @ turn @ source.matrix_world
    toes = lambda arm, posed: min(
        (arm.matrix_world @ (arm.pose.bones[n].head if posed else arm.data.bones[n].head_local)).z
        for n in ("mixamorig:LeftToeBase", "mixamorig:RightToeBase"))
    ground = float("inf")
    for frame in range(start, end + 1):
        scene.frame_set(frame)
        bpy.context.view_layer.update()
        ground = min(ground, toes(source, True))
    hips_target = rig.matrix_world @ rig.data.bones[ROOT].head_local
    hips_source = source.matrix_world @ source.data.bones[ROOT].head_local
    offset = mathutils.Vector((hips_target.x - hips_source.x, hips_target.y - hips_source.y,
                               toes(rig, False) - ground))
    source.matrix_world = mathutils.Matrix.Translation(offset) @ source.matrix_world
    # Standing height: where the hips are once the clip has settled on its last frame.
    scene.frame_set(end)
    bpy.context.view_layer.update()
    standing = (source.matrix_world @ source.pose.bones[ROOT].head).z

    # A clip that is not "In Place" carries the character across the ground. The game moves
    # the character itself, so the travel is taken out, evenly over the clip, and kept only
    # as the speed the clip was authored at: what the game plays it against.
    scene.frame_set(start)
    bpy.context.view_layer.update()
    travel = (source.matrix_world @ source.pose.bones[ROOT].head)
    scene.frame_set(end)
    bpy.context.view_layer.update()
    travel = (source.matrix_world @ source.pose.bones[ROOT].head) - travel
    travel.z = 0.0
    seconds = (end - start) / scene.render.fps
    authored_speed = travel.length / seconds if seconds > 0 else 0.0
    in_place = authored_speed < 0.5     # below this it is drift, not locomotion
    anchored = source.matrix_world.copy()

    for pb in bones:
        if pb.name not in source.pose.bones:
            continue
        c = pb.constraints.new('COPY_ROTATION')
        c.target, c.subtarget = source, pb.name
        c.target_space = c.owner_space = 'WORLD'
        if pb.name == ROOT:
            c = pb.constraints.new('COPY_LOCATION')
            c.target, c.subtarget = source, pb.name
            c.target_space = c.owner_space = 'WORLD'
            if airborne:
                c = pb.constraints.new('LIMIT_LOCATION')
                c.owner_space = 'WORLD'
                c.use_max_z, c.max_z = True, standing

    baked = {}
    for frame in range(first, last + 1):
        if not in_place:
            share = (frame - start) / (end - start)
            source.matrix_world = mathutils.Matrix.Translation(-travel * share) @ anchored
        scene.frame_set(frame)
        bpy.context.view_layer.update()
        baked[frame] = {pb.name: rig.convert_space(pose_bone=pb, matrix=pb.matrix,
                                                   from_space='POSE', to_space='LOCAL')
                        for pb in bones}

    for pb in bones:
        for c in list(pb.constraints):
            pb.constraints.remove(c)

    action = bpy.data.actions.new(name)
    action.use_fake_user = True
    if name in RELEASE:
        action["release"] = RELEASE[name]
    if not in_place:
        action["speed"] = round(authored_speed, 3)
        print("  ", name, "authored at", round(authored_speed, 2), "m/s; travel removed")
    rig.animation_data.action = action
    previous = {}
    # Keys from frame 0, so the clip starts at time zero; a held pose gets a second key.
    frames = [(frame - first, pose) for frame, pose in baked.items()]
    if first == last:
        frames.append((1, baked[first]))
    for frame, pose in frames:
        for pb in bones:
            pb.matrix_basis = pose[pb.name]
            q = pb.rotation_quaternion.copy()
            if pb.name in previous and previous[pb.name].dot(q) < 0:
                q.negate()      # keep the shortest path between keys
            pb.rotation_quaternion = q
            previous[pb.name] = q
            pb.keyframe_insert("rotation_quaternion", frame=frame, group=pb.name)
            if pb.name == ROOT:
                pb.keyframe_insert("location", frame=frame, group=pb.name)

    rig.animation_data.action = None
    for pb in bones:
        pb.matrix_basis = mathutils.Matrix.Identity(4)
    track = rig.animation_data.nla_tracks.new()
    track.name = name
    strip = track.strips.new(name, 0, action)
    if hasattr(strip, "action_slot") and action.slots:
        strip.action_slot = action.slots[0]

    for o in objs:
        bpy.data.objects.remove(o, do_unlink=True)
    bpy.data.actions.remove(source_action)

scene.frame_start = 0
scene.frame_set(0)
if BLEND:
    bpy.ops.wm.save_as_mainfile(filepath=os.path.abspath(BLEND))
bpy.ops.export_scene.gltf(
    filepath=os.path.abspath(OUT),
    export_format='GLB',
    export_animations=True,
    # Actions rather than NLA tracks: only this mode carries an action's custom properties
    # (the cast release time) into the glTF extras.
    export_animation_mode='ACTIONS',
    export_force_sampling=True,
    export_image_format='AUTO',
    export_extras=True,
)
print("DONE", OUT, [t.name for t in rig.animation_data.nla_tracks])
