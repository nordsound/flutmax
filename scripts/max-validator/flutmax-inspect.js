// flutmax-inspect.js
// .maxpat structural validator for v8.codebox (Max 9)
//
// Validates:
//   - JSON structure (patcher root, boxes array)
//   - Box fields (id, maxclass, numinlets, numoutlets, patching_rect)
//   - Box ID uniqueness
//   - newobj boxes have text field
//   - Connection validity (existing box IDs, inlet/outlet ranges)
//
// Usage: receives "validate <filepath> <requestId>" from node.script
// Returns: outlet(0, "result", jsonString) with validation results

// ---------------------------------------------------------------------------
// Read file contents using Max File object
// ---------------------------------------------------------------------------
function readFileContents(filepath) {
	const f = new File(filepath, "read");
	if (!f.isopen) {
		return { ok: false, error: "Could not open file: " + filepath };
	}

	let content = "";
	let line;
	while ((line = f.readline()) !== null && f.position < f.eof) {
		content += line + "\n";
	}
	if (line !== null) {
		content += line;
	}
	f.close();

	return { ok: true, content };
}

// ---------------------------------------------------------------------------
// Build box lookup map: id -> box data
// ---------------------------------------------------------------------------
function buildBoxMap(boxes) {
	const map = {};
	const duplicates = [];

	for (const entry of boxes) {
		const box = entry.box;
		if (!box) continue;
		const id = box.id;
		if (id && map[id]) {
			duplicates.push(id);
		}
		if (id) {
			map[id] = box;
		}
	}

	return { map, duplicates };
}

// ---------------------------------------------------------------------------
// Valid maxclass allowlist
// ---------------------------------------------------------------------------
const VALID_MAXCLASSES = new Set([
	"newobj", "inlet", "outlet", "comment", "message",
	"button", "toggle", "flonum", "number", "slider", "dial",
	"gain~", "ezdac~", "ezadc~", "scope~", "spectroscope~",
	"meter~", "number~", "bpatcher", "panel",
	"live.gain~", "live.dial", "live.slider", "live.toggle",
	"live.button", "live.numbox", "live.menu", "live.text", "live.tab",
	"multislider", "matrixctrl", "kslider", "nslider",
	"umenu", "textedit", "fpic", "pictctrl", "swatch",
	"attrui", "preset", "dropfile", "textbutton",
	"v8.codebox", "codebox",
]);

// ---------------------------------------------------------------------------
// Validate box structure
// ---------------------------------------------------------------------------
function validateBoxes(boxes) {
	const errors = [];
	const requiredFields = ["id", "maxclass", "numinlets", "numoutlets", "patching_rect"];

	for (let i = 0; i < boxes.length; i++) {
		const entry = boxes[i];
		if (!entry.box) {
			errors.push({ type: "missing_field", box_id: `boxes[${i}]`, message: "Box entry missing 'box' wrapper" });
			continue;
		}
		const box = entry.box;
		const boxId = box.id || `boxes[${i}]`;

		for (const field of requiredFields) {
			if (box[field] === undefined || box[field] === null) {
				errors.push({ type: "missing_field", box_id: boxId, message: `Box missing required field '${field}'` });
			}
		}

		if (!VALID_MAXCLASSES.has(box.maxclass)) {
			// Not an error — just means it's a custom external or unknown object
		}

		if (box.maxclass === "newobj" && (!box.text || box.text === "")) {
			errors.push({ type: "missing_text", box_id: boxId, message: "newobj box missing 'text' field" });
		}
	}

	return errors;
}

// ---------------------------------------------------------------------------
// Validate connections (patchlines)
// ---------------------------------------------------------------------------
function validateConnections(lines, boxMap) {
	const errors = [];

	for (let i = 0; i < lines.length; i++) {
		const entry = lines[i];
		if (!entry.patchline) {
			errors.push({ type: "missing_field", box_id: `lines[${i}]`, message: "Line entry missing 'patchline' wrapper" });
			continue;
		}

		const { source, destination } = entry.patchline;
		if (!source || !destination) {
			errors.push({ type: "missing_field", box_id: `lines[${i}]`, message: "Patchline missing 'source' or 'destination'" });
			continue;
		}

		const [srcId, srcOutlet] = source;
		const [dstId, dstInlet] = destination;

		if (!boxMap[srcId]) {
			errors.push({ type: "invalid_connection", box_id: srcId, message: `Source box '${srcId}' not found` });
		} else if (srcOutlet !== undefined && srcOutlet >= boxMap[srcId].numoutlets) {
			errors.push({ type: "outlet_out_of_range", box_id: srcId, message: `Source outlet ${srcOutlet} >= numoutlets ${boxMap[srcId].numoutlets} for box '${srcId}'` });
		}

		if (!boxMap[dstId]) {
			errors.push({ type: "invalid_connection", box_id: dstId, message: `Destination box '${dstId}' not found` });
		} else if (dstInlet !== undefined && dstInlet >= boxMap[dstId].numinlets) {
			errors.push({ type: "inlet_out_of_range", box_id: dstId, message: `Destination inlet ${dstInlet} >= numinlets ${boxMap[dstId].numinlets} for box '${dstId}'` });
		}
	}

	return errors;
}

// ---------------------------------------------------------------------------
// Main validation function
// ---------------------------------------------------------------------------
function validate(filepath, requestId) {
	post(`flutmax-inspect: validate id=${requestId} path=${filepath}\n`);

	const result = { id: requestId, status: "ok", errors: [], warnings: [], boxes_checked: 0, lines_checked: 0 };

	const fileResult = readFileContents(filepath);
	if (!fileResult.ok) {
		result.status = "error";
		result.errors.push({ type: "json_error", box_id: null, message: fileResult.error });
		sendResult(result);
		return;
	}

	let json;
	try {
		json = JSON.parse(fileResult.content);
	} catch (e) {
		result.status = "error";
		result.errors.push({ type: "json_error", box_id: null, message: `JSON parse error: ${e.message}` });
		sendResult(result);
		return;
	}

	if (!json.patcher) {
		result.status = "error";
		result.errors.push({ type: "missing_patcher", box_id: null, message: "No 'patcher' root key" });
		sendResult(result);
		return;
	}

	const boxes = json.patcher.boxes;
	if (!boxes || !Array.isArray(boxes)) {
		result.status = "error";
		result.errors.push({ type: "missing_field", box_id: null, message: "No 'boxes' array" });
		sendResult(result);
		return;
	}

	result.boxes_checked = boxes.length;

	const { map: boxMap, duplicates } = buildBoxMap(boxes);
	for (const dup of duplicates) {
		result.errors.push({ type: "duplicate_id", box_id: dup, message: `Duplicate box ID '${dup}'` });
	}

	result.errors.push(...validateBoxes(boxes));

	const lines = json.patcher.lines || [];
	result.lines_checked = lines.length;
	result.errors.push(...validateConnections(lines, boxMap));

	if (result.errors.length > 0) {
		result.status = "error";
	}

	post(`flutmax-inspect: done - ${result.errors.length} errors, ${result.warnings.length} warnings\n`);
	sendResult(result);
}

// ---------------------------------------------------------------------------
// Send result back to node.script
// ---------------------------------------------------------------------------
function sendResult(result) {
	outlet(0, "result", JSON.stringify(result));
}
