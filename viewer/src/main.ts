import "./styles.css";

const app = document.querySelector<HTMLElement>("#app");

if (!app) {
  throw new Error("Missing application root");
}

const view = new URLSearchParams(window.location.search).get("view");
app.className = view === "widget" ? "widget-shell" : "detail-shell";
app.textContent = view === "widget" ? "Parley is starting…" : "Parley Conversation Viewer";

