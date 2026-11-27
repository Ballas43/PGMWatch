const readline = require("readline");
let TikTokLiveConnection;
try {
  ({ TikTokLiveConnection } = require("tiktok-live-connector"));
} catch {
  TikTokLiveConnection = null;
}

const connections = new Map();
const desiredRooms = new Map(); // channelId -> { username, attempts }
const pendingTimers = new Map();

function emit(obj) {
  process.stdout.write(JSON.stringify(obj) + "\n");
}

function scheduleReconnect(channelId) {
  const desired = desiredRooms.get(channelId);
  if (!desired) return;
  if (pendingTimers.has(channelId)) {
    clearTimeout(pendingTimers.get(channelId));
    pendingTimers.delete(channelId);
  }
  const attempt = desired.attempts;
  desired.attempts = Math.min(attempt + 1, 6);
  const baseMs = Math.min(60000, 15000 * Math.pow(2, Math.min(attempt, 2)));
  const jitterMs = Math.floor(Math.random() * 4000);
  const timer = setTimeout(() => {
    pendingTimers.delete(channelId);
    const cur = desiredRooms.get(channelId);
    if (cur) subscribeRoom(channelId, cur.username);
  }, baseMs + jitterMs);
  pendingTimers.set(channelId, timer);
}

function subscribeRoom(channelId, username) {
  if (!TikTokLiveConnection) {
    emit({
      type: "error",
      channel_id: channelId,
      message: "tiktok-live-connector npm package not installed in sidecar-stats",
    });
    return;
  }

  const cleanUser = String(username || "").replace(/^@/, "").trim();
  if (!cleanUser) {
    emit({ type: "error", channel_id: channelId, message: "Missing TikTok username" });
    return;
  }

  if (connections.has(channelId)) {
    try {
      connections.get(channelId).disconnect();
    } catch {}
    connections.delete(channelId);
  }

  let ccv = null;
  let likes = null;
  let gifts = 0;
  let chatCount = 0;
  let isCurrent = () => connections.get(channelId) === conn;

  const conn = new TikTokLiveConnection(cleanUser, {
    processInitialData: true,
    enableExtendedGiftInfo: false,
  });

  conn.on("roomUser", (data) => {
    if (!isCurrent()) return;
    const nextCcv = Number(data?.viewerCount ?? data?.total ?? data?.viewer_count ?? data?.user_count);
    if (Number.isFinite(nextCcv)) {
      ccv = nextCcv;
    }
    emit({ type: "stats", channel_id: channelId, ccv, likes, gifts, chat_count: chatCount });
  });

  conn.on("like", (data) => {
    if (!isCurrent()) return;
    if (typeof data?.totalLikeCount === "number") {
      likes = data.totalLikeCount;
      emit({ type: "stats", channel_id: channelId, ccv, likes, gifts, chat_count: chatCount });
    }
  });

  conn.on("gift", (data) => {
    if (!isCurrent()) return;
    gifts += Number(data?.repeatCount ?? 1);
    emit({ type: "stats", channel_id: channelId, ccv, likes, gifts, chat_count: chatCount });
  });

  conn.on("chat", (data) => {
    if (!isCurrent()) return;
    const text = String(data?.comment || "").trim();
    if (!text) return;
    chatCount += 1;
    emit({
      type: "chat",
      channel_id: channelId,
      viewer_id: String(data?.uniqueId || data?.userId || "anon"),
      text,
    });
  });

  conn.on("disconnected", () => {
    if (!isCurrent()) return;
    connections.delete(channelId);
    emit({ type: "error", channel_id: channelId, message: "TikTok room disconnected (reconnecting...)" });
    scheduleReconnect(channelId);
  });

  connections.set(channelId, conn);

  conn.connect()
    .then((st) => {
      if (!isCurrent()) return;
      const desired = desiredRooms.get(channelId);
      if (desired) desired.attempts = 0;
      const ri = st?.roomInfo?.data || st?.roomInfo || {};
      const initialCcv = Number(ri?.user_count ?? ri?.stats?.user_count);
      if (Number.isFinite(initialCcv)) {
        ccv = initialCcv;
        emit({ type: "stats", channel_id: channelId, ccv, likes, gifts, chat_count: chatCount });
      }
    })
    .catch((err) => {
      if (!isCurrent()) return;
      connections.delete(channelId);
      const msg = String(err?.message || err || "TikTok connect failed");
      process.stderr.write(`[tiktok-sidecar] ${cleanUser}: ${msg}\n`);
      emit({
        type: "error",
        channel_id: channelId,
        message: msg,
      });
      scheduleReconnect(channelId);
    });
}

const rl = readline.createInterface({ input: process.stdin, crlfDelay: Infinity });
rl.on("line", (line) => {
  try {
    const msg = JSON.parse(line);
    if (msg.cmd === "subscribe") {
      desiredRooms.set(msg.channel_id, { username: msg.username, attempts: 0 });
      if (pendingTimers.has(msg.channel_id)) {
        clearTimeout(pendingTimers.get(msg.channel_id));
        pendingTimers.delete(msg.channel_id);
      }
      const delayMs = (connections.size + pendingTimers.size) * 2500; // staggered connects to avoid rate-limit bursts
      const timer = setTimeout(() => {
        pendingTimers.delete(msg.channel_id);
        const cur = desiredRooms.get(msg.channel_id);
        if (cur) subscribeRoom(msg.channel_id, cur.username);
      }, delayMs);
      pendingTimers.set(msg.channel_id, timer);
    } else if (msg.cmd === "unsubscribe") {
      desiredRooms.delete(msg.channel_id);
      if (pendingTimers.has(msg.channel_id)) {
        clearTimeout(pendingTimers.get(msg.channel_id));
        pendingTimers.delete(msg.channel_id);
      }
      if (connections.has(msg.channel_id)) {
        try {
          connections.get(msg.channel_id).disconnect();
        } catch {}
        connections.delete(msg.channel_id);
      }
    }
  } catch {}
});
