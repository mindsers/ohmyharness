// A model, for tests: it plays a script, and writes down everything it was sent.
//
// Every harness omh supports talks to it in its own protocol — Anthropic
// Messages (Claude Code), OpenAI Responses (Codex), OpenAI Chat Completions
// (opencode and omp) — and every one of them was measured completing a turn
// against these exact event sequences. What is left out of each is left out
// because the harness was measured not needing it.
//
// The script is `$FAKE_DIR/script.json`: an array of steps. A request that
// offers tools is the agent asking what to do next, and takes the next step;
// once the script runs out it is told it is done. A request that offers no
// tools — a title, a summary — is answered with plain text and takes nothing,
// so a harness's housekeeping cannot shift the script under the test.
//
//   { "tool":   { "name": "Bash", "args": { "command": "ls" } } }
//   { "custom": { "name": "apply_patch", "input": "*** Begin Patch\n…" } }
//   { "text":   "anything" }
//
// Every request is a line in `$FAKE_DIR/requests.jsonl`, body and all: that
// log is what the model saw, which is what a test asserts on. Run as the
// harness's HTTPS proxy too, it refuses every CONNECT and logs it, so traffic
// meant for anywhere else is a line in the same file instead of a request that
// left the sandbox.
import fs from "node:fs";
import http from "node:http";
import path from "node:path";

const port = Number(process.env.FAKE_PORT || 8787);
const dir = process.env.FAKE_DIR || "/tmp/fake";
const log = path.join(dir, "requests.jsonl");
const script = JSON.parse(fs.readFileSync(path.join(dir, "script.json"), "utf8"));
const DONE = "FAKE_DONE";
let n = 0;
let step = 0;

const record = (entry) => fs.appendFileSync(log, JSON.stringify({ n: ++n, ...entry }) + "\n");

function protocol(url) {
  if (/\/messages(\?|$)/.test(url) && !/count_tokens/.test(url)) return "anthropic";
  if (/\/responses(\?|$)/.test(url)) return "responses";
  if (/\/chat\/completions(\?|$)/.test(url)) return "chat";
  return null;
}

function offersTools(body) {
  return Array.isArray(body.tools) && body.tools.length > 0;
}

// The next step for a request that offers tools; plain text for one that
// offers none.
function reply(body) {
  if (!offersTools(body)) return { text: "FAKE_TEXT" };
  const next = script[step];
  if (next === undefined) return { text: DONE };
  step += 1;
  return next;
}

function sse(res, event, data) {
  res.write((event ? `event: ${event}\n` : "") + `data: ${typeof data === "string" ? data : JSON.stringify(data)}\n\n`);
}

function anthropic(res, body, plan) {
  const id = `toolu_fake${n}`;
  const block = plan.tool
    ? { type: "tool_use", id, name: plan.tool.name, input: plan.tool.args }
    : { type: "text", text: plan.text };
  const stop = plan.tool ? "tool_use" : "end_turn";
  const usage = { input_tokens: 1, output_tokens: 1 };
  if (!body.stream) {
    res.writeHead(200, { "content-type": "application/json" });
    return res.end(JSON.stringify({ id: `msg_fake${n}`, type: "message", role: "assistant", model: body.model, content: [block], stop_reason: stop, stop_sequence: null, usage }));
  }
  res.writeHead(200, { "content-type": "text/event-stream" });
  sse(res, "message_start", { type: "message_start", message: { id: `msg_fake${n}`, type: "message", role: "assistant", model: body.model, content: [], stop_reason: null, stop_sequence: null, usage } });
  if (plan.tool) {
    sse(res, "content_block_start", { type: "content_block_start", index: 0, content_block: { ...block, input: {} } });
    sse(res, "content_block_delta", { type: "content_block_delta", index: 0, delta: { type: "input_json_delta", partial_json: JSON.stringify(plan.tool.args) } });
  } else {
    sse(res, "content_block_start", { type: "content_block_start", index: 0, content_block: { type: "text", text: "" } });
    sse(res, "content_block_delta", { type: "content_block_delta", index: 0, delta: { type: "text_delta", text: plan.text } });
  }
  sse(res, "content_block_stop", { type: "content_block_stop", index: 0 });
  sse(res, "message_delta", { type: "message_delta", delta: { stop_reason: stop, stop_sequence: null }, usage: { output_tokens: 1 } });
  sse(res, "message_stop", { type: "message_stop" });
  res.end();
}

function responses(res, body, plan) {
  const id = `resp_fake${n}`;
  const item = plan.custom
    ? { type: "custom_tool_call", id: `ctc_fake${n}`, call_id: `call_fake${n}`, name: plan.custom.name, input: plan.custom.input, status: "completed" }
    : plan.tool
      ? { type: "function_call", id: `fc_fake${n}`, call_id: `call_fake${n}`, name: plan.tool.name, arguments: JSON.stringify(plan.tool.args), status: "completed" }
      : { type: "message", id: `msg_fake${n}`, role: "assistant", status: "completed", content: [{ type: "output_text", text: plan.text, annotations: [] }] };
  const usage = { input_tokens: 1, input_tokens_details: { cached_tokens: 0 }, output_tokens: 1, output_tokens_details: { reasoning_tokens: 0 }, total_tokens: 2 };
  res.writeHead(200, { "content-type": "text/event-stream" });
  sse(res, "response.created", { type: "response.created", response: { id, object: "response", status: "in_progress", model: body.model, output: [] } });
  sse(res, "response.output_item.done", { type: "response.output_item.done", output_index: 0, item });
  sse(res, "response.completed", { type: "response.completed", response: { id, object: "response", status: "completed", model: body.model, output: [item], usage } });
  res.end();
}

function chat(res, body, plan) {
  const base = { id: `chatcmpl-fake${n}`, object: "chat.completion.chunk", created: Math.floor(Date.now() / 1000), model: body.model };
  const call = plan.tool && { id: `call_fake${n}`, type: "function", function: { name: plan.tool.name, arguments: JSON.stringify(plan.tool.args) } };
  const finish = plan.tool ? "tool_calls" : "stop";
  const usage = { prompt_tokens: 1, completion_tokens: 1, total_tokens: 2 };
  if (!body.stream) {
    const message = plan.tool ? { role: "assistant", content: null, tool_calls: [call] } : { role: "assistant", content: plan.text };
    res.writeHead(200, { "content-type": "application/json" });
    return res.end(JSON.stringify({ ...base, object: "chat.completion", choices: [{ index: 0, message, finish_reason: finish }], usage }));
  }
  res.writeHead(200, { "content-type": "text/event-stream" });
  const delta = plan.tool ? { role: "assistant", tool_calls: [{ index: 0, ...call }] } : { role: "assistant", content: plan.text };
  sse(res, null, { ...base, choices: [{ index: 0, delta, finish_reason: null }] });
  // Without a finish reason opencode asks again, and again: 1,395 requests in
  // a minute, measured.
  sse(res, null, { ...base, choices: [{ index: 0, delta: {}, finish_reason: finish }], usage });
  sse(res, null, "[DONE]");
  res.end();
}

const server = http.createServer((req, res) => {
  req.on("error", () => {});
  res.on("error", () => {});
  let raw = "";
  req.on("data", (chunk) => (raw += chunk));
  req.on("end", () => {
    let body = {};
    try {
      body = raw ? JSON.parse(raw) : {};
    } catch {
      // Not JSON; logged as an empty body, and answered as a miss below.
    }
    const proto = req.method === "POST" ? protocol(req.url) : null;
    const plan = proto ? reply(body) : null;
    record({ method: req.method, url: req.url, proto, plan, body });
    if (plan) {
      if (proto === "anthropic") return anthropic(res, body, plan);
      if (proto === "responses") return responses(res, body, plan);
      return chat(res, body, plan);
    }
    if (req.method === "POST" && /count_tokens/.test(req.url)) {
      res.writeHead(200, { "content-type": "application/json" });
      return res.end('{"input_tokens":1}');
    }
    if (req.method === "GET" && /\/models(\?|$)/.test(req.url)) {
      res.writeHead(200, { "content-type": "application/json" });
      return res.end(JSON.stringify({ object: "list", data: [{ id: "fake-model", object: "model", created: 0, owned_by: "fake" }] }));
    }
    res.writeHead(404, { "content-type": "application/json" });
    res.end('{"error":{"type":"not_found","message":"the fake model has no such route"}}');
  });
});

server.on("connect", (req, socket) => {
  socket.on("error", () => {});
  record({ method: "CONNECT", url: req.url });
  socket.end("HTTP/1.1 403 Forbidden\r\n\r\n");
});

server.listen(port, "127.0.0.1", () => fs.writeFileSync(path.join(dir, "ready"), String(port)));
