const readline = require("readline");
let TikTokLiveConnection;
try {
  ({ TikTokLiveConnection } = require("tiktok-live-connector"));
} catch {
  TikTokLiveConnection = null;
}

const connections = new Map();

function emit(obj) {
  process.stdout.write(JSON.stringify(obj) + "\n");
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

  const conn = new TikTokLiveConnection(cleanUser, {
    processInitialData: true,
    enableExtendedGiftInfo: false,
  });

  conn.on("roomUser", (data) => {
    const nextCcv = Number(data?.viewerCount ?? data?.total ?? data?.viewer_count ?? data?.user_count);
    if (Number.isFinite(nextCcv)) {
      ccv = nextCcv;
    }
    emit({ type: "stats", channel_id: channelId, ccv, likes, gifts, chat_count: chatCount });
  });

  conn.on("like", (data) => {
    if (typeof data?.totalLikeCount === "number") {
      likes = data.totalLikeCount;
      emit({ type: "stats", channel_id: channelId, ccv, likes, gifts, chat_count: chatCount });
    }
  });

  conn.on("gift", (data) => {
    gifts += Number(data?.repeatCount ?? 1);
    emit({ type: "stats", channel_id: channelId, ccv, likes, gifts, chat_count: chatCount });
  });

  conn.on("chat", (data) => {
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
    emit({ type: "error", channel_id: channelId, message: "TikTok room disconnected" });
  });

  conn.connect()
    .then((st) => {
      const ri = st?.roomInfo?.data || st?.roomInfo || {};
      const initialCcv = Number(ri?.user_count ?? ri?.stats?.user_count);
      if (Number.isFinite(initialCcv)) {
        ccv = initialCcv;
        emit({ type: "stats", channel_id: channelId, ccv, likes, gifts, chat_count: chatCount });
      }
    })
    .catch((err) => {
      emit({
        type: "error",
        channel_id: channelId,
        message: String(err?.message || err || "TikTok connect failed"),
      });
    });

  connections.set(channelId, conn);
}

const pendingTimers = new Map();
const rl = readline.createInterface({ input: process.stdin, crlfDelay: Infinity });
rl.on("line", (line) => {
  try {
    const msg = JSON.parse(line);
    if (msg.cmd === "subscribe") {
      if (pendingTimers.has(msg.channel_id)) {
        clearTimeout(pendingTimers.get(msg.channel_id));
        pendingTimers.delete(msg.channel_id);
      }
      const delayMs = (connections.size + pendingTimers.size) * 1500; // staggered reconnects
      const timer = setTimeout(() => {
        pendingTimers.delete(msg.channel_id);
        subscribeRoom(msg.channel_id, msg.username);
      }, delayMs);
      pendingTimers.set(msg.channel_id, timer);
    } else if (msg.cmd === "unsubscribe") {
      if (pendingTimers.has(msg.channel_id)) {
        clearTimeout(pendingTimers.get(msg.channel_id));
        pendingTimers.delete(msg.channel_id);
      }
      if (connections.has(msg.channel_id)) {
        connections.get(msg.channel_id).disconnect();
        connections.delete(msg.channel_id);
      }
    }
  } catch {}
});
