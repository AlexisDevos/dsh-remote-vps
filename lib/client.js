window.__ModuleLoader__.load({
	id: "dsh-remote-vps",
	factory: (require) => {
		var module = { exports: {} };
		var exports = module.exports;
		Object.defineProperty(exports, Symbol.toStringTag, { value: "Module" });
		var react = require("react");

		var name = "dsh-remote-vps-ui";
		var inject = ["slots"];

		var CSS = [
			".ar-wrap{display:flex;flex-direction:column;gap:16px;width:100%;max-width:640px}",
			".ar-hint{color:var(--dsw-alias-label-secondary,#888);font-size:13px;line-height:20px}",
			".ar-hint.err{color:#f85149}",
			".ar-card{display:flex;flex-direction:column;gap:8px;padding:12px 14px;border:1px solid var(--dsw-alias-border,#333);border-radius:12px;background:var(--dsw-alias-bg-layer-2,#fff)}",
			".ar-card-head{display:flex;align-items:center;gap:10px}",
			".ar-card-main{flex:1;min-width:0}",
			".ar-card-title{font-weight:600;font-size:14px}",
			".ar-card-sub{color:var(--dsw-alias-label-secondary,#888);font-size:12px;overflow:hidden;text-overflow:ellipsis;white-space:nowrap}",
			".ar-badge{font-size:12px;padding:2px 8px;border-radius:999px;white-space:nowrap}",
			".ar-badge.ok{background:rgba(46,160,67,.16);color:#2ea043}",
			".ar-badge.err{background:rgba(248,81,73,.16);color:#f85149}",
			".ar-badge.wait{background:rgba(139,148,158,.2);color:#8b949e}",
			".ar-badge.active{background:rgba(56,139,253,.16);color:#388bfd}",
			".ar-btn{cursor:pointer;border:1px solid var(--dsw-alias-border,#333);background:transparent;color:var(--dsw-alias-label-primary,#111);border-radius:8px;padding:6px 12px;font-size:13px}",
			".ar-btn:hover{background:var(--dsw-alias-interactive-bg-hover,rgba(0,0,0,.05))}",
			".ar-btn:disabled{opacity:.5;cursor:default}",
			".ar-btn.primary{background:var(--dsw-alias-brand,#2f6fed);border-color:transparent;color:#fff}",
			".ar-btn.danger{color:#f85149}",
			".ar-form{display:flex;flex-direction:column;gap:10px;border:1px dashed var(--dsw-alias-border,#333);border-radius:12px;padding:14px}",
			".ar-row{display:flex;gap:10px}",
			".ar-field{display:flex;flex-direction:column;gap:4px;flex:1}",
			".ar-field label{font-size:12px;color:var(--dsw-alias-label-secondary,#888)}",
			".ar-field input{box-sizing:border-box;width:100%;padding:8px 10px;border:1px solid var(--dsw-alias-border,#333);border-radius:8px;background:var(--dsw-alias-bg-layer-1,#fafafa);color:var(--dsw-alias-label-primary,#111);font-size:13px}",
			".ar-actions{display:flex;gap:8px;flex-wrap:wrap;align-items:center}",
			".ar-section-title{font-size:15px;font-weight:600}"
		].join("\n");

		if (typeof document !== "undefined") {
			var tagId = "dsh-remote-vps/settings.css";
			if (!document.getElementById(tagId)) {
				var style = document.createElement("style");
				style.id = tagId;
				style.textContent = CSS;
				document.head.appendChild(style);
			}
		}

		var API = "/dsh-remote-vps/state";

		function apply(ctx) {
			var slots = ctx.get("slots");
			if (!slots) return;

			slots.inject("settings.section", () => slots.register({
				name: "settings.section",
				id: "dsh-remote-vps",
				order: 100,
				label: "VPS distants"
			}, (props) => react.createElement(Section, { close: props.close })));
		}

		function Section(props) {
			var state = react.useState({ connections: [], active: "", statuses: [], loading: true, error: "", saving: false });
			var st = state[0];
			var setSt = state[1];

			var editing = react.useState(null);
			var editState = editing[0];
			var setEditState = editing[1];
			var showForm = react.useState(false);
			var showFormState = showForm[0];
			var setShowFormState = showForm[1];

			var load = react.useCallback(function () {
				fetch(API).then(function (r) { return r.json(); }).then(function (d) {
					if (d && d.ok) {
						setSt(function (prev) { return Object.assign({}, prev, { connections: d.connections || [], active: d.active || "", statuses: d.statuses || [], loading: false, error: "" }); });
					} else {
						setSt(function (prev) { return Object.assign({}, prev, { loading: false, error: (d && d.message) ? d.message : "lecture refusée" }); });
					}
				}).catch(function (err) {
					setSt(function (prev) { return Object.assign({}, prev, { loading: false, error: "Pont local injoignable : " + String(err && err.message ? err.message : err) }); });
				});
			}, []);

			react.useEffect(function () {
				load();
				var timer = setInterval(load, 5000);
				return function () { clearInterval(timer); };
			}, [load]);

			var post = function (payload, after) {
				setSt(function (prev) { return Object.assign({}, prev, { saving: true }); });
				fetch(API, {
					method: "POST",
					headers: { "Content-Type": "application/json" },
					body: JSON.stringify(payload)
				}).then(function (r) { return r.json(); }).then(function (d) {
					if (d && d.ok) {
						setSt(function (prev) { return Object.assign({}, prev, { connections: d.connections || [], active: d.active || "", statuses: d.statuses || [], saving: false, error: "" }); });
						if (after) after();
					} else {
						setSt(function (prev) { return Object.assign({}, prev, { saving: false, error: (d && d.message) ? d.message : "écriture refusée" }); });
					}
				}).catch(function (err) {
					setSt(function (prev) { return Object.assign({}, prev, { saving: false, error: "Pont local injoignable : " + String(err && err.message ? err.message : err) }); });
				});
			};

			var conns = st.connections;
			var active = st.active;
			var statuses = st.statuses;

			var statusFor = function (id) {
				for (var i = 0; i < statuses.length; i++) if (statuses[i].id === id) return statuses[i];
				return null;
			};

			var saveConn = function (conn) {
				var dup = null;
				for (var i = 0; i < conns.length; i++) {
					var c = conns[i];
					if (c.id !== conn.id && c.host === conn.host && (c.port || 22) === (conn.port || 22)) dup = c.name || c.host;
				}
				if (dup !== null) {
					setSt(function (prev) { return Object.assign({}, prev, { error: "Doublon : hôte " + conn.host + ":" + (conn.port || 22) + " déjà configuré (« " + dup + " »)." }); });
					return;
				}
				var list = conns.slice();
				var idx = -1;
				for (var i = 0; i < list.length; i++) if (list[i].id === conn.id) idx = i;
				if (idx >= 0) list[idx] = conn; else list.push(conn);
				post({ connections: list, active: active, testTarget: "" }, function () {
					setEditState(null);
					setShowFormState(false);
				});
			};

			var removeConn = function (id) {
				if (!window.confirm("Supprimer cette connexion ?")) return;
				post({ connections: conns.filter(function (c) { return c.id !== id; }), active: active, testTarget: "" });
			};

			var setActive = function (id) {
				post({ connections: conns, active: id, testTarget: "" });
			};

			var testTarget = function (id) {
				post({ connections: conns, active: active, testTarget: id || "" });
			};

			var copySsh = function (conn) {
				var cmd = "ssh -p " + (conn.port || 22) + " " + (conn.user || "root") + "@" + (conn.host || "host");
				try {
					navigator.clipboard.writeText(cmd).then(function () {
						setSt(function (prev) { return Object.assign({}, prev, { error: "Commande copiée : " + cmd }); });
					}).catch(function () {
						setSt(function (prev) { return Object.assign({}, prev, { error: "Copie impossible : " + cmd }); });
					});
				} catch (e2) {
					setSt(function (prev) { return Object.assign({}, prev, { error: "Copie impossible : " + cmd }); });
				}
			};

			var e = react.createElement;

			var rows = conns.map(function (conn) {
				var stt = statusFor(conn.id);
				var badge;
				if (stt && stt.ok) badge = e("span", { className: "ar-badge ok", key: "b" }, "OK " + stt.ms + "ms");
				else if (stt && !stt.ok) badge = e("span", { className: "ar-badge err", key: "b", title: stt.error || "" }, "Échec");
				else badge = e("span", { className: "ar-badge wait", key: "b" }, "non testé");
				var activeBadge = conn.id === active ? e("span", { className: "ar-badge active", key: "a" }, "actif") : null;
				var checked = stt && stt.checkedAt ? "Vérifié à " + new Date(stt.checkedAt).toLocaleTimeString() : "Jamais vérifié";
				return e("div", { className: "ar-card", key: conn.id },
					e("div", { className: "ar-card-head" },
						e("div", { className: "ar-card-main" },
							e("div", { className: "ar-card-title" }, conn.name || conn.host),
							e("div", { className: "ar-card-sub" }, (conn.user || "root") + "@" + conn.host + ":" + (conn.port || 22) + "  —  " + (conn.baseDir || "(baseDir par défaut)"))
						),
						activeBadge,
						badge
					),
					e("div", { className: "ar-actions" },
						conn.id === active ? null : e("button", { className: "ar-btn", onClick: function () { setActive(conn.id); } }, "Activer"),
						e("button", { className: "ar-btn", onClick: function () { setEditState(conn); setShowFormState(true); } }, "Modifier"),
						e("button", { className: "ar-btn", onClick: function () { testTarget(conn.id); } }, "Tester"),
						e("button", { className: "ar-btn", onClick: function () { copySsh(conn); } }, "Copier SSH"),
						e("button", { className: "ar-btn danger", onClick: function () { removeConn(conn.id); } }, "Suppr."),
						e("span", { className: "ar-hint" }, checked)
					)
				);
			});

			var form = null;
			if (showFormState) {
				var draft = editState || { id: "conn-" + Date.now(), name: "", host: "", user: "root", port: 22, keyFile: "", baseDir: "" };
				var upd = function (k) {
					return function (ev) { setEditState(Object.assign({}, draft, { [k]: ev.target.value })); };
				};
				form = e("div", { className: "ar-form" },
					e("div", { className: "ar-row" },
						e("div", { className: "ar-field" }, e("label", null, "Nom"), e("input", { value: draft.name || "", placeholder: "Astrée VPS", onChange: upd("name") })),
						e("div", { className: "ar-field" }, e("label", null, "Hôte (IP Tailscale ou MagicDNS) — obligatoire"), e("input", { value: draft.host || "", placeholder: "mon-vps ou 100.x.y.z", onChange: upd("host") }))
					),
					e("div", { className: "ar-row" },
						e("div", { className: "ar-field" }, e("label", null, "Utilisateur"), e("input", { value: String(draft.user || "root"), onChange: upd("user") })),
						e("div", { className: "ar-field" }, e("label", null, "Port"), e("input", { value: String(draft.port == null ? 22 : draft.port), onChange: upd("port") })),
						e("div", { className: "ar-field" }, e("label", null, "Clé SSH (optionnel)"), e("input", { value: draft.keyFile || "", placeholder: "~/.ssh/id_ed25519", onChange: upd("keyFile") }))
					),
					e("div", { className: "ar-field" }, e("label", null, "baseDir (répertoire du projet sur le VPS)"), e("input", { value: draft.baseDir || "", placeholder: "/chemin/vers/le/projet", onChange: upd("baseDir") })),
					e("div", { className: "ar-actions" },
						e("button", {
							className: "ar-btn primary",
							disabled: st.saving || !(draft.host || "").trim(),
							onClick: function () {
								saveConn({ id: draft.id, name: (draft.name || draft.host || draft.id).trim(), host: (draft.host || "").trim(), user: (draft.user || "root").trim(), port: parseInt(String(draft.port), 10) || 22, keyFile: (draft.keyFile || "").trim(), baseDir: (draft.baseDir || "").trim() });
							}
						}, st.saving ? "Enregistrement…" : "Enregistrer"),
						e("button", { className: "ar-btn", onClick: function () { setEditState(null); setShowFormState(false); } }, "Annuler")
					)
				);
			}

			var statusLine = st.loading
				? "Chargement des connexions…"
				: st.error
					? st.error
					: "Pont local connecté · " + conns.length + " connexion(s)";

			return e("div", { className: "ar-wrap" },
				e("div", { className: "ar-section-title" }, "Connexions VPS"),
				e("div", { className: st.error ? "ar-hint err" : "ar-hint" }, statusLine),
				e("div", { className: "ar-hint" },
					"Chaque connexion pilote un VPS via SSH (multiplexé). Utilise l'IP Tailscale (100.x.y.z) ou le nom MagicDNS : le trafic reste sur ton tailnet. baseDir est le répertoire de travail distant des outils read/write/edit/glob/grep/bash."
				),
				rows.length ? rows : (st.loading ? null : e("div", { className: "ar-hint" }, "Aucune connexion enregistrée : ajoute ton VPS ci-dessous (les outils distants afficheront une erreur tant qu'aucune connexion n'est définie).")),
				e("div", { className: "ar-actions" },
					e("button", {
						className: "ar-btn primary",
						onClick: function () { setEditState(null); setShowFormState(true); }
					}, "+ Ajouter un VPS"),
					conns.length ? e("button", { className: "ar-btn", onClick: function () { testTarget(""); } }, "Tester les connexions") : null
				),
				form
			);
		}

		exports.name = name;
		exports.inject = inject;
		exports.apply = apply;
		return module.exports;
	}
});
