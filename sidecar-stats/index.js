const readline = require("readline");
let TikTokLiveConnection;
try {
  ({ TikTokLiveConnection } = require("tiktok-live-connector"));
} catch {
  TikTokLiveConnection = null;
}

const connections = new Map();
const desiredRooms = new Map(); // channelId -> { username, sessionId, attempts }
const pendingTimers = new Map();
const restPollTimers = new Map(); // channelId -> interval handle for direct TikTok fetchRoomInfo fallback

function emit(obj) {
  process.stdout.write(JSON.stringify(obj) + "\n");
}

function stopRestPoll(channelId) {
  if (restPollTimers.has(channelId)) {
    clearInterval(restPollTimers.get(channelId));
    restPollTimers.delete(channelId);
  }
}

function startRestPoll(channelId, username, sessionId) {
  stopRestPoll(channelId);
  const pollOnce = async () => {
    const cur = desiredRooms.get(channelId);
    if (!cur || !TikTokLiveConnection) {
      stopRestPoll(channelId);
      return;
    }
    try {
      const opts = { processInitialData: false, enableExtendedGiftInfo: false };
      if (sessionId && String(sessionId).trim()) {
        opts.sessionId = String(sessionId).trim();
      }
      const probe = new TikTokLiveConnection(username, opts);
      const ri = await probe.fetchRoomInfo();
      const data = ri?.data || ri || {};
      const ccv = Number(data?.user_count ?? data?.stats?.user_count);
      const totalViews = Number(data?.stats?.total_user ?? data?.stats?.enter_count);
      const rawLikes = data?.like_count ?? data?.stats?.like_count;
      const rawShares = data?.stats?.share_count ?? data?.share_count;
      if (Number.isFinite(ccv)) {
        emit({
          type: "stats",
          channel_id: channelId,
          ccv,
          total_views: Number.isFinite(totalViews) && totalViews > 0 ? totalViews : null,
          likes: typeof rawLikes === "number" && rawLikes > 0 ? rawLikes : null,
          shares: typeof rawShares === "number" && rawShares > 0 ? rawShares : null,
          gifts: null,
          chat_count: null,
        });
      }
    } catch {
      // ignore transient roomInfo errors; next tick retries
    }
  };
  void pollOnce();
  const handle = setInterval(() => void pollOnce(), 15000);
  restPollTimers.set(channelId, handle);
}

function scheduleReconnect(channelId, isRateLimited = false) {
  const desired = desiredRooms.get(channelId);
  if (!desired) return;
  if (pendingTimers.has(channelId)) {
    clearTimeout(pendingTimers.get(channelId));
    pendingTimers.delete(channelId);
  }
  const attempt = desired.attempts;
  desired.attempts = Math.min(attempt + 1, 6);
  // If EulerStream sign server is rate-limited, back off 10 minutes while direct fetchRoomInfo
  // REST polling keeps live Viewers & Total Views updating every 15s without hitting EulerStream.
  const baseMs = isRateLimited
    ? 600000
    : Math.min(120000, 15000 * Math.pow(2, Math.min(attempt, 3)));
  const jitterMs = Math.floor(Math.random() * 5000);
  const timer = setTimeout(() => {
    pendingTimers.delete(channelId);
    const cur = desiredRooms.get(channelId);
    if (cur) subscribeRoom(channelId, cur.username, cur.sessionId);
  }, baseMs + jitterMs);
  pendingTimers.set(channelId, timer);
}

function subscribeRoom(channelId, username, sessionId) {
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
  let totalViews = null;
  let likes = 0;
  let shares = 0;
  let gifts = 0;
  const chatTimestamps = [];
  let isCurrent = () => connections.get(channelId) === conn;

  const currentChatRatePerMin = () => {
    const cutoff = Date.now() - 60000;
    while (chatTimestamps.length > 0 && chatTimestamps[0] < cutoff) {
      chatTimestamps.shift();
    }
    return chatTimestamps.length;
  };

  const opts = {
    processInitialData: true,
    enableExtendedGiftInfo: false,
  };
  if (sessionId && String(sessionId).trim()) {
    opts.sessionId = String(sessionId).trim();
  }
  const conn = new TikTokLiveConnection(cleanUser, opts);

  const emitStats = () => {
    emit({
      type: "stats",
      channel_id: channelId,
      ccv,
      total_views: totalViews,
      likes,
      shares,
      gifts,
      chat_count: currentChatRatePerMin(),
    });
  };

  conn.on("roomUser", (data) => {
    if (!isCurrent()) return;
    const nextCcv = Number(data?.viewerCount ?? data?.total ?? data?.viewer_count ?? data?.user_count);
    if (Number.isFinite(nextCcv)) {
      ccv = nextCcv;
    }
    emitStats();
  });

  conn.on("like", (data) => {
    if (!isCurrent()) return;
    if (typeof data?.totalLikeCount === "number") {
      likes = data.totalLikeCount;
      emitStats();
    }
  });

  conn.on("share", () => {
    if (!isCurrent()) return;
    shares = (shares ?? 0) + 1;
    emitStats();
  });

  conn.on("gift", (data) => {
    if (!isCurrent()) return;
    gifts += Number(data?.repeatCount ?? 1);
    emitStats();
  });

  conn.on("chat", (data) => {
    if (!isCurrent()) return;
    const text = String(data?.comment || "").trim();
    if (!text) return;
    chatTimestamps.push(Date.now());
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
    startRestPoll(channelId, cleanUser, sessionId);
    scheduleReconnect(channelId, false);
  });

  connections.set(channelId, conn);

  conn.connect()
    .then((st) => {
      if (!isCurrent()) return;
      stopRestPoll(channelId);
      const desired = desiredRooms.get(channelId);
      if (desired) desired.attempts = 0;
      const ri = st?.roomInfo?.data || st?.roomInfo || {};
      const initialCcv = Number(ri?.user_count ?? ri?.stats?.user_count);
      const initialViews = Number(ri?.stats?.total_user ?? ri?.stats?.enter_count);
      const rawLikes = ri?.like_count ?? ri?.stats?.like_count;
      const rawShares = ri?.stats?.share_count ?? ri?.share_count;
      let changed = false;
      if (Number.isFinite(initialCcv)) {
        ccv = initialCcv;
        changed = true;
      }
      if (Number.isFinite(initialViews) && initialViews > 0) {
        totalViews = initialViews;
        changed = true;
      }
      if (typeof rawLikes === "number" && rawLikes > 0) {
        likes = rawLikes;
        changed = true;
      }
      if (typeof rawShares === "number" && rawShares > 0) {
        shares = rawShares;
        changed = true;
      }
      if (changed) {
        emitStats();
      }
    })
    .catch((err) => {
      if (!isCurrent()) return;
      connections.delete(channelId);
      const msg = String(err?.message || err || "TikTok connect failed");
      const rateLimited = /rate.?limit|too many connections/i.test(msg);
      process.stderr.write(`[tiktok-sidecar] ${cleanUser}: ${msg}\n`);
      // Immediately start direct TikTok fetchRoomInfo REST polling (zero EulerStream dependency)
      // so live Viewers & Total Views stay populated even when EulerStream is rate-limited.
      startRestPoll(channelId, cleanUser, sessionId);
      scheduleReconnect(channelId, rateLimited);
    });
}

const rl = readline.createInterface({ input: process.stdin, crlfDelay: Infinity });
rl.on("line", (line) => {
  try {
    const msg = JSON.parse(line);
    if (msg.cmd === "subscribe") {
      desiredRooms.set(msg.channel_id, {
        username: msg.username,
        sessionId: msg.session_id || null,
        attempts: 0,
      });
      if (pendingTimers.has(msg.channel_id)) {
        clearTimeout(pendingTimers.get(msg.channel_id));
        pendingTimers.delete(msg.channel_id);
      }
      const delayMs = (connections.size + pendingTimers.size) * 2500; // staggered connects to avoid rate-limit bursts
      const timer = setTimeout(() => {
        pendingTimers.delete(msg.channel_id);
        const cur = desiredRooms.get(msg.channel_id);
        if (cur) subscribeRoom(msg.channel_id, cur.username, cur.sessionId);
      }, delayMs);
      pendingTimers.set(msg.channel_id, timer);
    } else if (msg.cmd === "unsubscribe") {
      desiredRooms.delete(msg.channel_id);
      stopRestPoll(msg.channel_id);
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
