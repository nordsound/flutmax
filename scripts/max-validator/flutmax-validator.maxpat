{
  "patcher": {
    "appversion": {
      "architecture": "x64",
      "major": 8,
      "minor": 6,
      "modernui": 1,
      "revision": 0
    },
    "assistshowspatchername": 0,
    "autosave": 0,
    "bglocked": 0,
    "bottomtoolbarpinned": 0,
    "boxanimatetime": 200,
    "boxes": [
      {
        "box": {
          "id": "obj-1",
          "maxclass": "message",
          "numinlets": 2,
          "numoutlets": 1,
          "outlettype": [
            ""
          ],
          "patching_rect": [
            100.0,
            50.0,
            50.0,
            22.0
          ],
          "text": "script start",
          "varname": "msg_script"
        }
      },
      {
        "box": {
          "id": "obj-2",
          "maxclass": "newobj",
          "numinlets": 0,
          "numoutlets": 0,
          "patching_rect": [
            100.0,
            120.0,
            80.0,
            22.0
          ],
          "text": "loadbang",
          "varname": "lb"
        }
      },
      {
        "box": {
          "id": "obj-3",
          "maxclass": "newobj",
          "numinlets": 1,
          "numoutlets": 1,
          "outlettype": [
            ""
          ],
          "patching_rect": [
            100.0,
            190.0,
            80.0,
            22.0
          ],
          "text": "node.script flutmax-validate-server.mjs",
          "varname": "server"
        }
      },
      {
        "box": {
          "code": "// flutmax-inspect-v8.js\n// .maxpat structural validator for v8.codebox (Max 9)\n//\n// Migrated from flutmax-inspect.js (Max [js] object) to v8.codebox.\n// API is identical — File, JSON.parse, outlet() all work in v8.codebox.\n//\n// Validates:\n//   - JSON structure (patcher root, boxes array)\n//   - Box fields (id, maxclass, numinlets, numoutlets, patching_rect)\n//   - Box ID uniqueness\n//   - newobj boxes have text field\n//   - Connection validity (existing box IDs, inlet/outlet ranges)\n//\n// Usage: receives \"validate <filepath> <requestId>\" from node.script\n// Returns: outlet(0, \"result\", jsonString) with validation results\n\n// ---------------------------------------------------------------------------\n// Read file contents using Max File object\n// ---------------------------------------------------------------------------\nfunction readFileContents(filepath) {\n\tconst f = new File(filepath, \"read\");\n\tif (!f.isopen) {\n\t\treturn { ok: false, error: \"Could not open file: \" + filepath };\n\t}\n\n\tlet content = \"\";\n\tlet line;\n\twhile ((line = f.readline()) !== null && f.position < f.eof) {\n\t\tcontent += line + \"\\n\";\n\t}\n\tif (line !== null) {\n\t\tcontent += line;\n\t}\n\tf.close();\n\n\treturn { ok: true, content };\n}\n\n// ---------------------------------------------------------------------------\n// Build box lookup map: id -> box data\n// ---------------------------------------------------------------------------\nfunction buildBoxMap(boxes) {\n\tconst map = {};\n\tconst duplicates = [];\n\n\tfor (const entry of boxes) {\n\t\tconst box = entry.box;\n\t\tif (!box) continue;\n\t\tconst id = box.id;\n\t\tif (id && map[id]) {\n\t\t\tduplicates.push(id);\n\t\t}\n\t\tif (id) {\n\t\t\tmap[id] = box;\n\t\t}\n\t}\n\n\treturn { map, duplicates };\n}\n\n// ---------------------------------------------------------------------------\n// Valid maxclass allowlist\n// ---------------------------------------------------------------------------\nconst VALID_MAXCLASSES = new Set([\n\t\"newobj\", \"inlet\", \"outlet\", \"comment\", \"message\",\n\t\"button\", \"toggle\", \"flonum\", \"number\", \"slider\", \"dial\",\n\t\"gain~\", \"ezdac~\", \"ezadc~\", \"scope~\", \"spectroscope~\",\n\t\"meter~\", \"number~\", \"bpatcher\", \"panel\",\n\t\"live.gain~\", \"live.dial\", \"live.slider\", \"live.toggle\",\n\t\"live.button\", \"live.numbox\", \"live.menu\", \"live.text\", \"live.tab\",\n\t\"multislider\", \"matrixctrl\", \"kslider\", \"nslider\",\n\t\"umenu\", \"textedit\", \"fpic\", \"pictctrl\", \"swatch\",\n\t\"attrui\", \"preset\", \"dropfile\", \"textbutton\",\n\t\"v8.codebox\", \"codebox\",\n]);\n\n// ---------------------------------------------------------------------------\n// Validate box structure\n// ---------------------------------------------------------------------------\nfunction validateBoxes(boxes) {\n\tconst errors = [];\n\tconst requiredFields = [\"id\", \"maxclass\", \"numinlets\", \"numoutlets\", \"patching_rect\"];\n\n\tfor (let i = 0; i < boxes.length; i++) {\n\t\tconst entry = boxes[i];\n\t\tif (!entry.box) {\n\t\t\terrors.push({ type: \"missing_field\", box_id: `boxes[${i}]`, message: \"Box entry missing 'box' wrapper\" });\n\t\t\tcontinue;\n\t\t}\n\t\tconst box = entry.box;\n\t\tconst boxId = box.id || `boxes[${i}]`;\n\n\t\tfor (const field of requiredFields) {\n\t\t\tif (box[field] === undefined || box[field] === null) {\n\t\t\t\terrors.push({ type: \"missing_field\", box_id: boxId, message: `Box missing required field '${field}'` });\n\t\t\t}\n\t\t}\n\n\t\tif (!VALID_MAXCLASSES.has(box.maxclass)) {\n\t\t\t// Not an error — just means it's a custom external or unknown object\n\t\t}\n\n\t\tif (box.maxclass === \"newobj\" && (!box.text || box.text === \"\")) {\n\t\t\terrors.push({ type: \"missing_text\", box_id: boxId, message: \"newobj box missing 'text' field\" });\n\t\t}\n\t}\n\n\treturn errors;\n}\n\n// ---------------------------------------------------------------------------\n// Validate connections (patchlines)\n// ---------------------------------------------------------------------------\nfunction validateConnections(lines, boxMap) {\n\tconst errors = [];\n\n\tfor (let i = 0; i < lines.length; i++) {\n\t\tconst entry = lines[i];\n\t\tif (!entry.patchline) {\n\t\t\terrors.push({ type: \"missing_field\", box_id: `lines[${i}]`, message: \"Line entry missing 'patchline' wrapper\" });\n\t\t\tcontinue;\n\t\t}\n\n\t\tconst { source, destination } = entry.patchline;\n\t\tif (!source || !destination) {\n\t\t\terrors.push({ type: \"missing_field\", box_id: `lines[${i}]`, message: \"Patchline missing 'source' or 'destination'\" });\n\t\t\tcontinue;\n\t\t}\n\n\t\tconst [srcId, srcOutlet] = source;\n\t\tconst [dstId, dstInlet] = destination;\n\n\t\tif (!boxMap[srcId]) {\n\t\t\terrors.push({ type: \"invalid_connection\", box_id: srcId, message: `Source box '${srcId}' not found` });\n\t\t} else if (srcOutlet !== undefined && srcOutlet >= boxMap[srcId].numoutlets) {\n\t\t\terrors.push({ type: \"outlet_out_of_range\", box_id: srcId, message: `Source outlet ${srcOutlet} >= numoutlets ${boxMap[srcId].numoutlets} for box '${srcId}'` });\n\t\t}\n\n\t\tif (!boxMap[dstId]) {\n\t\t\terrors.push({ type: \"invalid_connection\", box_id: dstId, message: `Destination box '${dstId}' not found` });\n\t\t} else if (dstInlet !== undefined && dstInlet >= boxMap[dstId].numinlets) {\n\t\t\terrors.push({ type: \"inlet_out_of_range\", box_id: dstId, message: `Destination inlet ${dstInlet} >= numinlets ${boxMap[dstId].numinlets} for box '${dstId}'` });\n\t\t}\n\t}\n\n\treturn errors;\n}\n\n// ---------------------------------------------------------------------------\n// Main validation function\n// ---------------------------------------------------------------------------\nfunction validate(filepath, requestId) {\n\tpost(`flutmax-inspect: validate id=${requestId} path=${filepath}\\n`);\n\n\tconst result = { id: requestId, status: \"ok\", errors: [], warnings: [], boxes_checked: 0, lines_checked: 0 };\n\n\tconst fileResult = readFileContents(filepath);\n\tif (!fileResult.ok) {\n\t\tresult.status = \"error\";\n\t\tresult.errors.push({ type: \"json_error\", box_id: null, message: fileResult.error });\n\t\tsendResult(result);\n\t\treturn;\n\t}\n\n\tlet json;\n\ttry {\n\t\tjson = JSON.parse(fileResult.content);\n\t} catch (e) {\n\t\tresult.status = \"error\";\n\t\tresult.errors.push({ type: \"json_error\", box_id: null, message: `JSON parse error: ${e.message}` });\n\t\tsendResult(result);\n\t\treturn;\n\t}\n\n\tif (!json.patcher) {\n\t\tresult.status = \"error\";\n\t\tresult.errors.push({ type: \"missing_patcher\", box_id: null, message: \"No 'patcher' root key\" });\n\t\tsendResult(result);\n\t\treturn;\n\t}\n\n\tconst boxes = json.patcher.boxes;\n\tif (!boxes || !Array.isArray(boxes)) {\n\t\tresult.status = \"error\";\n\t\tresult.errors.push({ type: \"missing_field\", box_id: null, message: \"No 'boxes' array\" });\n\t\tsendResult(result);\n\t\treturn;\n\t}\n\n\tresult.boxes_checked = boxes.length;\n\n\tconst { map: boxMap, duplicates } = buildBoxMap(boxes);\n\tfor (const dup of duplicates) {\n\t\tresult.errors.push({ type: \"duplicate_id\", box_id: dup, message: `Duplicate box ID '${dup}'` });\n\t}\n\n\tresult.errors.push(...validateBoxes(boxes));\n\n\tconst lines = json.patcher.lines || [];\n\tresult.lines_checked = lines.length;\n\tresult.errors.push(...validateConnections(lines, boxMap));\n\n\tif (result.errors.length > 0) {\n\t\tresult.status = \"error\";\n\t}\n\n\tpost(`flutmax-inspect: done - ${result.errors.length} errors, ${result.warnings.length} warnings\\n`);\n\tsendResult(result);\n}\n\n// ---------------------------------------------------------------------------\n// Send result back to node.script\n// ---------------------------------------------------------------------------\nfunction sendResult(result) {\n\toutlet(0, \"result\", JSON.stringify(result));\n}\n",
          "filename": "none",
          "id": "obj-4",
          "maxclass": "v8.codebox",
          "numinlets": 1,
          "numoutlets": 1,
          "outlettype": [
            ""
          ],
          "patching_rect": [
            100.0,
            260.0,
            200.0,
            100.0
          ],
          "text": "",
          "varname": "inspector"
        }
      }
    ],
    "classnamespace": "box",
    "default_fontface": 0,
    "default_fontname": "Arial",
    "default_fontsize": 12.0,
    "dependency_cache": [],
    "description": "",
    "devicewidth": 0.0,
    "digest": "",
    "enablehscroll": 1,
    "enablevscroll": 1,
    "fileversion": 1,
    "gridonopen": 1,
    "gridsize": [
      15.0,
      15.0
    ],
    "gridsnaponopen": 1,
    "lefttoolbarpinned": 0,
    "lines": [
      {
        "patchline": {
          "destination": [
            "obj-1",
            0
          ],
          "source": [
            "obj-2",
            0
          ]
        }
      },
      {
        "patchline": {
          "destination": [
            "obj-3",
            0
          ],
          "source": [
            "obj-1",
            0
          ]
        }
      },
      {
        "patchline": {
          "destination": [
            "obj-4",
            0
          ],
          "source": [
            "obj-3",
            0
          ]
        }
      },
      {
        "patchline": {
          "destination": [
            "obj-3",
            0
          ],
          "source": [
            "obj-4",
            0
          ]
        }
      }
    ],
    "objectsnaponopen": 1,
    "openinpresentation": 0,
    "rect": [
      100.0,
      100.0,
      640.0,
      480.0
    ],
    "righttoolbarpinned": 0,
    "statusbarvisible": 2,
    "style": "",
    "subpatcher_template": "",
    "tags": "",
    "tallnewobj": 0,
    "toolbars_unpinned_last_save": 0,
    "toolbarvisible": 1,
    "toptoolbarpinned": 0
  }
}