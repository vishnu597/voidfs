# SPDX-License-Identifier: Apache-2.0
"""Run by Blender, via app-matrix-free.py; only touches its newly-created test folder."""

import json
import sys
from pathlib import Path

import bpy


folder, phase, result = sys.argv[sys.argv.index("--") + 1:]
folder, result = Path(folder), Path(result)
project = folder / "scene café.blend"
object_name = "Cube café 東京"
report = {"phase": phase, "version": bpy.app.version_string, "checks": []}


def check(name, value):
    report["checks"].append({"name": name, "passed": bool(value)})
    if not value:
        raise AssertionError(name)


try:
    if phase == "create":
        bpy.context.preferences.filepaths.save_version = 1
        cube = bpy.data.objects["Cube"]
        cube.name = object_name
        cube["owner"] = "voidfs compatibility control"
        cube["revision"] = 1
        bpy.context.scene["title"] = "Texture and Unicode round trip"
        cube.location.x = 0
        cube.keyframe_insert(data_path="location", frame=1)
        cube.location.x = 0.5
        cube.keyframe_insert(data_path="location", frame=10)
        bpy.context.scene.frame_set(1)
        assets = folder / "assets"
        assets.mkdir()
        image = bpy.data.images.new("Texture café", width=2, height=2)
        image.pixels = [1, 0, 0, 1, 0, 1, 0, 1, 0, 0, 1, 1, 1, 1, 1, 1]
        image.filepath_raw = str(assets / "texture.png")
        image.file_format = "PNG"
        image.save()
        image.filepath = "//assets/texture.png"
        material = bpy.data.materials.new("External texture")
        material.use_nodes = True
        node = material.node_tree.nodes.new("ShaderNodeTexImage")
        node.image = image
        material.node_tree.links.new(node.outputs["Color"], material.node_tree.nodes["Principled BSDF"].inputs["Base Color"])
        cube.data.materials.clear()
        cube.data.materials.append(material)
        bpy.context.scene.render.engine = "CYCLES"
        bpy.context.scene.cycles.samples = 4
        bpy.context.scene.render.resolution_x = 64
        bpy.context.scene.render.resolution_y = 64
        bpy.context.scene.render.resolution_percentage = 100
        check("native first save", bpy.ops.wm.save_as_mainfile(filepath=str(project), check_existing=False) == {"FINISHED"})
    else:
        check("native fresh-process reopen", bpy.ops.wm.open_mainfile(filepath=str(project)) == {"FINISHED"})
        cube = bpy.data.objects[object_name]
        check("Unicode object and custom metadata", cube["owner"] == "voidfs compatibility control")
        check("geometry survives", len(cube.data.vertices) == 8 and len(cube.data.polygons) == 6)
        image = bpy.data.images["Texture café"]
        check("external texture relative path", image.filepath == "//assets/texture.png" and image.packed_file is None)
        image.reload()
        check("external texture decoded", tuple(image.size) == (2, 2) and list(image.pixels[:4]) == [1, 0, 0, 1])
        if phase == "edit":
            check("initial revision reopens", cube["revision"] == 1)
            bpy.context.preferences.filepaths.save_version = 1
            cube["revision"] = 2
            cube.scale = (1.25, 1.5, 1.75)
            check("native edited save", bpy.ops.wm.save_mainfile() == {"FINISHED"})
            check("native backup exists", project.with_suffix(".blend1").is_file())
        elif phase == "verify-initial":
            check("restored first revision reopens", cube["revision"] == 1 and tuple(cube.scale) == (1, 1, 1))
            bpy.context.scene.frame_set(10)
            check("restored animation reopens", abs(cube.location.x - 0.5) < 0.00001)
        elif phase == "verify":
            check("edited revision reopens", cube["revision"] == 2 and tuple(cube.scale) == (1.25, 1.5, 1.75))
            bpy.context.scene.frame_set(10)
            check("animation reopens", abs(cube.location.x - 0.5) < 0.00001)
            bpy.context.scene.frame_set(1)
            bpy.context.scene.render.filepath = str(folder / "render.png")
            check("render reopened scene", bpy.ops.render.render(write_still=True) == {"FINISHED"})
            check("render bytes written", (folder / "render.png").stat().st_size > 100)
            check("native backup reopen", bpy.ops.wm.open_mainfile(filepath=str(project.with_suffix(".blend1"))) == {"FINISHED"})
            check("backup retains first revision", bpy.data.objects[object_name]["revision"] == 1)
        else:
            raise ValueError(phase)
    report["passed"] = True
finally:
    result.write_text(json.dumps(report, indent=2, ensure_ascii=False) + "\n")
