// The launcher: the instances the person added and, for each, the products its manifest lists.
// Opening a product opens its own window on the product's host (the app's `product_open`). The page
// never sees a token: the commands return none, and `instances:changed` carries none.

import fr from "./i18n/fr.js";
import en from "./i18n/en.js";
import es from "./i18n/es.js";
import de from "./i18n/de.js";
import it from "./i18n/it.js";
import pl from "./i18n/pl.js";

const dictionaries = { fr, en, es, de, it, pl };
const lang = (navigator.language || "fr").split("-")[0].toLowerCase();
const words = dictionaries[lang] ?? fr;
/** A word of the launcher, French when the language misses it. */
const t = (key) => words[key] ?? fr[key] ?? key;

const { invoke } = window.__TAURI__.core;
const { listen } = window.__TAURI__.event;

const list = document.getElementById("instances");
const empty = document.getElementById("empty");
const keychain = document.getElementById("keychain");
const form = document.getElementById("add");
const input = document.getElementById("domain");
const error = document.getElementById("error");

function showError(code) {
  error.hidden = !code;
  error.textContent = code ? (fr[`err_${code}`] ? t(`err_${code}`) : t("err_failed")) : "";
}

function button(label, className, onClick) {
  const b = document.createElement("button");
  b.type = "button";
  b.className = className;
  b.textContent = label;
  b.addEventListener("click", onClick);
  return b;
}

async function run(command, args) {
  showError(null);
  try {
    await invoke(command, args);
  } catch (code) {
    showError(typeof code === "string" ? code : "failed");
  }
}

function render(instances) {
  list.replaceChildren();
  empty.hidden = instances.length > 0;
  keychain.hidden = instances.every((i) => i.vault_persistent);
  for (const instance of instances) {
    const item = document.createElement("li");
    item.className = "instance";
    const head = document.createElement("div");
    head.className = "head";
    const domain = document.createElement("span");
    domain.className = "domain";
    domain.textContent = instance.domain;
    const who = document.createElement("span");
    who.className = "who";
    who.textContent = instance.signing_in
      ? t("signing_in")
      : instance.signed_in
        ? instance.account ?? ""
        : t("signed_out");
    head.append(domain, who);
    item.append(head);

    const products = document.createElement("div");
    products.className = "products";
    for (const product of instance.products) {
      const open = button(t(product.name), "product", () =>
        run("product_open", { id: instance.id, product: product.name }),
      );
      open.title = product.url;
      products.append(open);
    }
    item.append(products);

    const actions = document.createElement("div");
    actions.className = "actions";
    if (instance.signed_in) {
      actions.append(button(t("sign_out"), "link", () => run("instance_sign_out", { id: instance.id })));
    } else {
      const signIn = button(t("sign_in"), "link", () => run("instance_sign_in", { id: instance.id, lang }));
      signIn.disabled = instance.signing_in;
      actions.append(signIn);
    }
    actions.append(
      button(t("remove"), "link", () => {
        if (confirm(t("confirm_remove"))) run("instance_remove", { id: instance.id });
      }),
    );
    item.append(actions);
    list.append(item);
  }
}

for (const element of document.querySelectorAll("[data-t]")) {
  element.textContent = t(element.dataset.t);
}
document.documentElement.lang = dictionaries[lang] ? lang : "fr";

form.addEventListener("submit", async (event) => {
  event.preventDefault();
  const submit = form.querySelector("button");
  submit.disabled = true;
  await run("instance_add", { address: input.value, lang });
  if (error.hidden) input.value = "";
  submit.disabled = false;
});

await listen("instances:changed", (event) => render(event.payload));
render(await invoke("instances_state"));
invoke("set_shell_labels", {
  labels: {
    open: t("open"),
    quit: t("quit"),
    closed_to_tray: t("closed_to_tray"),
    saved_title: t("saved_title"),
    saved_body: t("saved_body"),
  },
});
