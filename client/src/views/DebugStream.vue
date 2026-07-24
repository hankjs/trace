<script setup lang="ts">
import { ref } from "vue";
import { API_BASE } from "../config";

const logs = ref<string[]>([]);
const running = ref(false);

function log(msg: string) {
  logs.value.push(`[${new Date().toISOString().slice(11, 23)}] ${msg}`);
}

async function runMockTest() {
  logs.value = [];
  running.value = true;

  try {
    const { invoke, Channel } = await import("@tauri-apps/api/core");

    log(`Starting mock test`);

    const onEvent = new Channel<{ data: string; done: boolean }>();
    onEvent.onmessage = (event) => {
      if (event.done) {
        log(`✓ DONE received`);
      } else {
        log(`Event: "${event.data}"`);
      }
    };

    log("Calling invoke...");
    const startTime = Date.now();
    await invoke("llm_stream_test", { onEvent });
    log(`invoke resolved after ${Date.now() - startTime}ms`);
  } catch (err: any) {
    log(`ERROR: ${err.message || err}`);
  }

  running.value = false;
}

async function runRealTest() {
  logs.value = [];
  running.value = true;

  try {
    const { invoke, Channel } = await import("@tauri-apps/api/core");

    const token = localStorage.getItem("hank_client_token") || "";

    // Minimal LLM request
    const body = JSON.stringify({
      system: "你是一个作文老师，请用中文回答。",
      messages: [{ role: "user", content: [{ type: "text", text: "请写一篇500字的作文，题目是《春天的早晨》。" }] }],
      tools: [],
      max_tokens: 2048,
    });

    const url = `${API_BASE}/api/llm/completion`;

    log(`Starting real SSE test → ${url}`);

    let eventCount = 0;
    const onEvent = new Channel<{ data: string; done: boolean }>();
    onEvent.onmessage = (event) => {
      eventCount++;
      if (event.done) {
        log(`✓ DONE (total events: ${eventCount})`);
      } else {
        const preview = event.data.length > 80 ? event.data.slice(0, 80) + "..." : event.data;
        log(`#${eventCount} ${preview}`);
      }
    };

    log("Calling invoke...");
    const startTime = Date.now();
    await invoke("llm_stream", {
      req: { url, token, body },
      onEvent,
    });
    log(`invoke resolved after ${Date.now() - startTime}ms, events=${eventCount}`);
  } catch (err: any) {
    log(`ERROR: ${err.message || err}`);
  }

  running.value = false;
}
</script>

<template>
  <div class="p-6 max-w-3xl mx-auto">
    <h1 class="text-xl font-bold mb-4">Tauri SSE Stream Debug</h1>

    <div class="flex gap-3 mb-4">
      <button
        class="px-4 py-2 bg-blue-600 text-white rounded hover:bg-blue-700 disabled:opacity-50"
        :disabled="running"
        @click="runMockTest"
      >
        Mock Test (10 events × 200ms)
      </button>
      <button
        class="px-4 py-2 bg-green-600 text-white rounded hover:bg-green-700 disabled:opacity-50"
        :disabled="running"
        @click="runRealTest"
      >
        Real LLM Test
      </button>
    </div>

    <div class="bg-gray-900 text-green-300 font-mono text-xs p-4 rounded h-[500px] overflow-y-auto">
      <div v-if="logs.length === 0" class="text-gray-500">点击按钮开始测试...</div>
      <div v-for="(line, i) in logs" :key="i" class="whitespace-pre-wrap">{{ line }}</div>
    </div>
  </div>
</template>
