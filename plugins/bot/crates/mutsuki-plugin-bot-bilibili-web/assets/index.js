function escapeHtml(value) {
  return String(value ?? "")
    .replaceAll("&", "&amp;")
    .replaceAll("<", "&lt;")
    .replaceAll(">", "&gt;")
    .replaceAll('"', "&quot;");
}

function formatError(err) {
  const message = err && typeof err === "object" && err.message ? String(err.message) : "";
  return message.startsWith("extension ") || message.includes("rpc ")
    ? "操作失败，请稍后重试"
    : message || "操作失败，请稍后重试";
}

function formatTarget(target) {
  if (!target || typeof target !== "object") return "—";
  switch (target.type) {
    case "group":
      return `群 ${target.group_id}`;
    case "user":
      return `用户 ${target.user_id}`;
    case "guild_channel":
      return `频道 ${target.guild_id}/${target.channel_id}`;
    case "conversation":
      return `会话 ${target.conversation_id}`;
    case "platform_specific":
      return `${target.platform}:${target.kind}:${target.id}`;
    default:
      return "—";
  }
}

function safeHttpUrl(value) {
  if (value == null || value === "") return "";
  try {
    const parsed = new URL(String(value));
    if (parsed.protocol === "http:" || parsed.protocol === "https:") return parsed.href;
  } catch {
    return "";
  }
  return "";
}

const NOTIFY_LABELS = { live: "直播", dynamic: "动态", video: "视频" };
const NOTIFY_VALUES = { 直播: "live", 动态: "dynamic", 视频: "video" };

function formatNotifications(values) {
  return (values || [])
    .map((item) => NOTIFY_LABELS[String(item).toLowerCase()] || String(item))
    .join("、");
}

function parseNotifications(text) {
  return String(text || "")
    .split(/[,，、\s]+/)
    .map((item) => item.trim())
    .filter(Boolean)
    .map((item) => NOTIFY_VALUES[item] || item.toLowerCase());
}

function kv(label, value) {
  const row = document.createElement("li");
  row.innerHTML = `<span>${escapeHtml(label)}</span><span>${escapeHtml(value)}</span>`;
  return row;
}

function kvList(entries) {
  const list = document.createElement("ul");
  list.className = "kv";
  for (const [label, value] of entries) {
    if (value == null || value === "") continue;
    list.appendChild(kv(label, String(value)));
  }
  return list;
}

function section(title) {
  const el = document.createElement("section");
  el.className = "card";
  const h = document.createElement("h2");
  h.textContent = title;
  el.appendChild(h);
  return el;
}

function button(label, className = "") {
  const btn = document.createElement("button");
  btn.type = "button";
  btn.className = className || "ghost";
  btn.textContent = label;
  return btn;
}

function field(label, input) {
  const wrap = document.createElement("label");
  wrap.className = "form-field";
  wrap.innerHTML = `<span>${escapeHtml(label)}</span>`;
  wrap.appendChild(input);
  return wrap;
}

function textInput(placeholder = "", value = "") {
  const input = document.createElement("input");
  input.type = "text";
  input.placeholder = placeholder;
  input.value = value;
  return input;
}

/**
 * Mount Bilibili management panel into an overview content host.
 * @param {HTMLElement} host
 * @param {{ read: Function, write: Function }} rpc
 */
export function mountBilibiliPanel(host, rpc, events) {
  host.innerHTML = "";
  const root = document.createElement("div");
  root.className = "bilibili-panel stack";
  host.appendChild(root);

  const statusBox = section("登录");
  const qrBox = section("扫码登录");
  const listBox = section("订阅");
  const addBox = section("新增订阅");
  const bindBox = section("自助绑定");
  const msg = document.createElement("div");
  msg.className = "muted";
  const refreshButton = button("刷新", "ghost");
  const toolbar = document.createElement("div");
  toolbar.className = "toolbar";
  toolbar.append(refreshButton, msg);
  root.append(toolbar, statusBox, qrBox, listBox, addBox, bindBox);

  let loginPollTimer = null;
  let refreshTimer = null;
  let eventTimer = null;
  let refreshInFlight = null;
  let pendingRefresh = false;
  let lastRevision = 0;
  let opened = false;
  let disposed = false;
  let formsBuilt = false;
  let status = null;

  function setMessage(text, isError = false) {
    msg.className = isError ? "error-banner" : "muted";
    msg.textContent = text || "";
  }

  async function refreshStatus() {
    status = await rpc.read("bilibili", "status");
    statusBox.querySelectorAll(".kv, .actions, .hint").forEach((n) => n.remove());
    statusBox.append(
      kvList([
        ["状态", status.credential_loaded ? "已登录" : "未登录"],
        ["订阅管理", status.management_enabled ? "可用" : "未启用"],
        ["订阅", String(status.subscription_count ?? 0)],
      ]),
    );
    if (status.reason) {
      const hint = document.createElement("p");
      hint.className = "hint muted";
      hint.textContent = status.reason;
      statusBox.appendChild(hint);
    }
    const actions = document.createElement("div");
    actions.className = "actions";
    const clearBtn = button("清除凭据", "");
    clearBtn.onclick = async () => {
      if (!window.confirm("确认清除 B 站登录凭据？")) return;
      try {
        await rpc.write("bilibili", "credential.clear", { confirmed: true });
        setMessage("凭据已清除");
        await refreshAll();
      } catch (err) {
        setMessage(formatError(err), true);
      }
    };
    actions.appendChild(clearBtn);
    statusBox.appendChild(actions);

    addBox.hidden = !status.management_enabled;
    bindBox.hidden = !(status.management_enabled && status.allow_self_binding);
  }

  async function refreshList() {
    listBox.querySelectorAll(".sub-list, .empty, .preview").forEach((n) => n.remove());
    if (!status?.management_enabled) {
      listBox.appendChild(Object.assign(document.createElement("p"), {
        className: "empty muted",
        textContent: "管理未启用",
      }));
      return;
    }
    const body = await rpc.read("bilibili", "subscriptions.list", { is_admin: true });
    const items = body?.subscriptions || [];
    if (!items.length) {
      listBox.appendChild(Object.assign(document.createElement("p"), {
        className: "empty muted",
        textContent: "暂无订阅",
      }));
      return;
    }
    const list = document.createElement("div");
    list.className = "sub-list stack";
    for (const item of items) {
      const card = document.createElement("article");
      card.className = "card card--outlined";
      const heading = document.createElement("h3");
      heading.textContent = `${formatTarget(item.target)} · UID ${item.uid}`;
      card.append(
        heading,
        kvList([
          ["通知", formatNotifications(item.notifications)],
          ["绑定", item.outbound_binding],
          ["暂停", item.paused ? "是" : "否"],
          item.owner_user_id ? ["所有者", item.owner_user_id] : null,
        ].filter(Boolean)),
      );
      const actions = document.createElement("div");
      actions.className = "actions";
      const pauseBtn = button(item.paused ? "恢复" : "暂停");
      pauseBtn.disabled = !status.management_enabled;
      pauseBtn.onclick = async () => {
        try {
          await rpc.write("bilibili", "subscriptions.set_paused", {
            selector: item.subscription_id,
            paused: !item.paused,
            is_admin: true,
          });
          await refreshAll();
        } catch (err) {
          setMessage(formatError(err), true);
        }
      };
      const previewBtn = button("预览");
      previewBtn.onclick = async () => {
        try {
          const cardView = await rpc.read("bilibili", "subscriptions.preview", {
            selector: item.subscription_id,
            is_admin: true,
          });
          let preview = listBox.querySelector(".preview");
          if (!preview) {
            preview = document.createElement("div");
            preview.className = "preview card card--outlined";
            listBox.appendChild(preview);
          }
          preview.replaceChildren();
          const titleEl = document.createElement("strong");
          titleEl.textContent = cardView.title ?? "";
          const descEl = document.createElement("div");
          descEl.className = "muted";
          descEl.textContent = cardView.description ?? "";
          preview.append(titleEl, descEl);
          const href = safeHttpUrl(cardView.url);
          if (href) {
            const link = document.createElement("a");
            link.href = href;
            link.target = "_blank";
            link.rel = "noreferrer";
            link.textContent = String(cardView.url ?? href);
            preview.appendChild(link);
          } else if (cardView.url) {
            const urlText = document.createElement("div");
            urlText.textContent = String(cardView.url);
            preview.appendChild(urlText);
          }
        } catch (err) {
          setMessage(formatError(err), true);
        }
      };
      const delBtn = button("删除");
      delBtn.disabled = !status.management_enabled;
      delBtn.onclick = async () => {
        if (!window.confirm(`确认删除订阅 ${formatTarget(item.target)}？`)) return;
        try {
          await rpc.write("bilibili", "subscriptions.unsubscribe", {
            subscription_id: item.subscription_id,
            confirmed: true,
          });
          await refreshAll();
        } catch (err) {
          setMessage(formatError(err), true);
        }
      };
      actions.append(pauseBtn, previewBtn, delBtn);
      card.appendChild(actions);
      list.appendChild(card);
    }
    listBox.appendChild(list);
  }

  function buildQrUi() {
    qrBox.querySelectorAll(".qr-body, .actions").forEach((n) => n.remove());
    const body = document.createElement("div");
    body.className = "qr-body";
    const img = document.createElement("img");
    img.alt = "Bilibili 登录二维码";
    img.hidden = true;
    img.style.maxWidth = "256px";
    const state = document.createElement("p");
    state.className = "muted";
    state.textContent = "点击开始扫码登录";
    body.append(img, state);
    const actions = document.createElement("div");
    actions.className = "actions";
    const startBtn = button("开始扫码登录", "");
    startBtn.onclick = async () => {
      try {
        if (loginPollTimer) clearInterval(loginPollTimer);
        const started = await rpc.write("bilibili", "login.start");
        img.src = `data:image/png;base64,${started.qr_png_base64}`;
        img.hidden = false;
        state.textContent = "等待扫码…";
        loginPollTimer = setInterval(async () => {
          try {
            const polled = await rpc.read("bilibili", "login.poll");
            state.textContent = polled.message || polled.status;
            if (polled.status === "confirmed" || polled.status === "expired") {
              clearInterval(loginPollTimer);
              loginPollTimer = null;
              if (polled.status === "confirmed") {
                img.hidden = true;
                await refreshAll();
              }
            }
          } catch (err) {
            clearInterval(loginPollTimer);
            loginPollTimer = null;
            setMessage(formatError(err), true);
          }
        }, 2000);
      } catch (err) {
        setMessage(formatError(err), true);
      }
    };
    actions.appendChild(startBtn);
    qrBox.append(body, actions);
  }

  function buildAddForm() {
    addBox.querySelectorAll("form").forEach((n) => n.remove());
    const form = document.createElement("form");
    form.className = "stack";
    const idInput = textInput("订阅名称");
    const uidInput = textInput("B 站 UID");
    const bindingInput = textInput("绑定名");
    const groupInput = textInput("群号");
    const notifyInput = textInput("直播,动态,视频", "直播,动态,视频");
    form.append(
      field("订阅名称", idInput),
      field("B 站 UID", uidInput),
      field("绑定名", bindingInput),
      field("推送群", groupInput),
      field("通知类型", notifyInput),
    );
    const submit = button("创建订阅", "");
    submit.onclick = async (event) => {
      event.preventDefault();
      try {
        const notifications = parseNotifications(notifyInput.value);
        await rpc.write("bilibili", "subscriptions.subscribe", {
          subscription_id: idInput.value.trim(),
          uid: Number(uidInput.value),
          outbound_binding: bindingInput.value.trim(),
          notifications,
          target: { type: "group", group_id: groupInput.value.trim() },
        });
        setMessage("订阅已写入");
        await refreshAll();
      } catch (err) {
        setMessage(formatError(err), true);
      }
    };
    form.appendChild(submit);
    addBox.appendChild(form);
  }

  function buildBindForm() {
    bindBox.querySelectorAll("form, .bind-result").forEach((n) => n.remove());
    const form = document.createElement("form");
    form.className = "stack";
    const operatorInput = textInput("操作者");
    const uidInput = textInput("B 站 UID");
    const groupInput = textInput("群号");
    const result = document.createElement("div");
    result.className = "bind-result muted";
    form.append(
      field("操作者", operatorInput),
      field("B 站 UID", uidInput),
      field("推送群", groupInput),
    );
    const startBtn = button("发起绑定", "");
    startBtn.onclick = async (event) => {
      event.preventDefault();
      try {
        const challenge = await rpc.write("bilibili", "binding.start", {
          operator_user_id: operatorInput.value.trim(),
          uid: Number(uidInput.value),
        });
        result.textContent = `请把 ${challenge.code} 写入 ${challenge.name} 的个性签名，然后点验证。`;
      } catch (err) {
        setMessage(formatError(err), true);
      }
    };
    const verifyBtn = button("验证绑定", "");
    verifyBtn.onclick = async (event) => {
      event.preventDefault();
      try {
        const verified = await rpc.write("bilibili", "binding.verify", {
          operator_user_id: operatorInput.value.trim(),
          platform: "web",
          target: { type: "group", group_id: groupInput.value.trim() },
        });
        if (verified.result === "signature_mismatch") {
          result.textContent = `验证未通过：个性签名中尚未找到 ${verified.code}`;
        } else {
          result.textContent = "绑定成功";
          await refreshAll();
        }
      } catch (err) {
        setMessage(formatError(err), true);
      }
    };
    const unbindBtn = button("解除自助绑定");
    unbindBtn.onclick = async (event) => {
      event.preventDefault();
      try {
        await rpc.write("bilibili", "binding.unbind", {
          operator_user_id: operatorInput.value.trim(),
        });
        setMessage("已解除绑定");
        await refreshAll();
      } catch (err) {
        setMessage(formatError(err), true);
      }
    };
    form.append(startBtn, verifyBtn, unbindBtn, result);
    bindBox.appendChild(form);
  }

  async function loadData() {
    await refreshStatus();
    if (!formsBuilt) {
      buildQrUi();
      buildAddForm();
      buildBindForm();
      formsBuilt = true;
    }
    await refreshList();
  }

  function scheduleRefresh() {
    clearTimeout(refreshTimer);
    if (!disposed && !document.hidden) refreshTimer = setTimeout(() => void refreshAll(), 60_000);
  }

  function refreshAll() {
    if (disposed) return Promise.resolve();
    if (refreshInFlight) {
      pendingRefresh = true;
      return refreshInFlight;
    }
    refreshInFlight = loadData()
      .catch((err) => {
        if (!disposed) setMessage(formatError(err), true);
      })
      .finally(() => {
        refreshInFlight = null;
        if (pendingRefresh) {
          pendingRefresh = false;
          void refreshAll();
        } else {
          scheduleRefresh();
        }
      });
    return refreshInFlight;
  }

  const visibility = () => {
    clearTimeout(refreshTimer);
    if (!document.hidden) void refreshAll();
  };
  const eventSubscription = events.subscribe("bilibili.changed", (payload) => {
    const revision = Number(payload?.revision || 0);
    if (revision <= lastRevision) return;
    lastRevision = revision;
    if (document.hidden) return;
    clearTimeout(eventTimer);
    eventTimer = setTimeout(() => void refreshAll(), 50);
  }, "runtime.read");
  const connectionSubscription = events.onStateChange?.((connection) => {
    if (connection !== "open" || document.hidden) return;
    if (opened) void refreshAll();
    opened = true;
  });
  refreshButton.onclick = () => void refreshAll();
  document.addEventListener("visibilitychange", visibility);
  void refreshAll();
  return {
    destroy() {
      disposed = true;
      if (loginPollTimer) clearInterval(loginPollTimer);
      clearTimeout(refreshTimer);
      clearTimeout(eventTimer);
      eventSubscription.dispose();
      connectionSubscription?.dispose();
      document.removeEventListener("visibilitychange", visibility);
    },
  };
}

export default {
  id: "bilibili",
  setup(ctx) {
    ctx.activities.register({
      id: "bot",
      label: "Bot",
      icon: "bot",
      order: 10,
      position: "top",
    });
    ctx.navigation.register({
      id: "bilibili.nav",
      activityId: "bot",
      pageId: "bilibili.page",
      label: "B站推送",
      order: 8,
      requiredCapability: "runtime.read",
    });
    ctx.pages.register({
      id: "bilibili.page",
      path: "/bilibili",
      title: "B站推送",
      pluginId: "mutsuki.bot.bilibili",
      component: {
        mount(el) {
          const panel = mountBilibiliPanel(el, ctx.rpc, ctx.events);
          return { dispose: () => panel?.destroy?.() };
        },
      },
      requiredCapability: "runtime.read",
    });
  },
};
