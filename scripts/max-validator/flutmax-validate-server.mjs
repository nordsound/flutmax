// flutmax-validate-server.mjs
// Production UDP server for .maxpat validation
//
// Architecture:
//   CLI --UDP 7401--> node.script --outlet--> js (File + JSON.parse + validation)
//   CLI <--UDP 7402-- node.script <--outlet-- js (results JSON)
//
// Commands:
//   {"cmd": "ping"}
//   {"cmd": "validate", "path": "/path/to/file.maxpat", "id": "req-001"}
//   {"cmd": "shutdown"}

import { default as Max } from "max-api";
import dgram from "node:dgram";

const UDP_LISTEN_PORT = 7401;
const UDP_REPLY_PORT = 7402;
const UDP_REPLY_HOST = "127.0.0.1";
const REQUEST_TIMEOUT_MS = 30000;

// Pending requests: id -> { timer, rinfo }
const pendingRequests = new Map();

// UDP sockets
const server = dgram.createSocket("udp4");
const replySocket = dgram.createSocket("udp4");

// ---------------------------------------------------------------------------
// Send a JSON response via UDP
// ---------------------------------------------------------------------------
function sendReply(data) {
	const json = JSON.stringify(data);
	const buf = Buffer.from(json);
	replySocket.send(buf, 0, buf.length, UDP_REPLY_PORT, UDP_REPLY_HOST, (err) => {
		if (err) {
			Max.post("UDP reply error: " + err.message);
		}
	});
}

// ---------------------------------------------------------------------------
// Handle results from js object (flutmax-inspect.js)
// ---------------------------------------------------------------------------
Max.addHandler("result", (resultJson) => {
	Max.post("Received validation result from js");

	try {
		const result = JSON.parse(resultJson);
		const id = result.id;

		// Clear timeout for this request
		if (pendingRequests.has(id)) {
			clearTimeout(pendingRequests.get(id).timer);
			pendingRequests.delete(id);
		}

		sendReply(result);
		Max.post("Result sent for request: " + id);
	} catch (e) {
		Max.post("Error processing result: " + e.message);
		sendReply({
			id: "unknown",
			status: "error",
			errors: [{ type: "internal_error", message: "Failed to process validation result: " + e.message }],
			warnings: [],
			boxes_checked: 0,
			lines_checked: 0
		});
	}
});

// ---------------------------------------------------------------------------
// Handle incoming UDP messages
// ---------------------------------------------------------------------------
server.on("message", (msg, rinfo) => {
	const raw = msg.toString();
	Max.post("UDP received from " + rinfo.address + ":" + rinfo.port + ": " + raw);

	let request;
	try {
		request = JSON.parse(raw);
	} catch (e) {
		Max.post("JSON parse error: " + e.message);
		sendReply({
			id: "unknown",
			status: "error",
			errors: [{ type: "json_error", message: "Invalid JSON in request: " + e.message }],
			warnings: [],
			boxes_checked: 0,
			lines_checked: 0
		});
		return;
	}

	const cmd = request.cmd;

	if (cmd === "ping") {
		Max.post("Ping received, sending pong");
		sendReply({ status: "pong" });

	} else if (cmd === "validate") {
		const path = request.path;
		const id = request.id;

		if (!path || !id) {
			Max.post("Validate request missing path or id");
			sendReply({
				id: id || "unknown",
				status: "error",
				errors: [{ type: "missing_field", message: "Validate request requires 'path' and 'id' fields" }],
				warnings: [],
				boxes_checked: 0,
				lines_checked: 0
			});
			return;
		}

		Max.post("Forwarding validate request: id=" + id + " path=" + path);

		// Set timeout for this request
		const timer = setTimeout(() => {
			if (pendingRequests.has(id)) {
				pendingRequests.delete(id);
				Max.post("Request timed out: " + id);
				sendReply({
					id: id,
					status: "error",
					errors: [{ type: "timeout", message: "Validation request timed out after " + REQUEST_TIMEOUT_MS + "ms" }],
					warnings: [],
					boxes_checked: 0,
					lines_checked: 0
				});
			}
		}, REQUEST_TIMEOUT_MS);

		pendingRequests.set(id, { timer: timer, rinfo: rinfo });

		// Forward to js object
		Max.outlet("validate", path, id);

	} else if (cmd === "shutdown") {
		Max.post("Shutdown requested, closing sockets...");
		sendReply({ status: "shutdown" });

		// Clear all pending request timers
		for (const [id, entry] of pendingRequests) {
			clearTimeout(entry.timer);
		}
		pendingRequests.clear();

		// Close sockets gracefully
		setTimeout(() => {
			server.close(() => {
				Max.post("UDP listen socket closed");
			});
			replySocket.close(() => {
				Max.post("UDP reply socket closed");
			});
			Max.post("Shutdown complete");
		}, 100);

	} else {
		Max.post("Unknown command: " + cmd);
		sendReply({
			id: request.id || "unknown",
			status: "error",
			errors: [{ type: "unknown_command", message: "Unknown command: " + cmd }],
			warnings: [],
			boxes_checked: 0,
			lines_checked: 0
		});
	}
});

server.on("error", (err) => {
	Max.post("UDP server error: " + err.message);
});

// ---------------------------------------------------------------------------
// Start server
// ---------------------------------------------------------------------------
server.bind(UDP_LISTEN_PORT, () => {
	Max.post("=== flutmax-validator ===");
	Max.post("UDP server listening on port " + UDP_LISTEN_PORT);
	Max.post("Reply port: " + UDP_REPLY_PORT);
	Max.post("Commands: ping, validate, shutdown");
	Max.post("Test: echo '{\"cmd\":\"ping\"}' | nc -u 127.0.0.1 " + UDP_LISTEN_PORT);
	Max.post("=========================");
});
