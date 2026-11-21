import "./styles.css";
import "./un-series.css";
import App from "./App.svelte";
import { mount } from "svelte";
import { waitLocale } from "svelte-i18n";
import { setupI18n } from "@usagi.network/un-i18n-svelte";

document.documentElement.dataset.unProduct = "motion";
document.documentElement.dataset.unDesign ??= "hybrid";

// i18n bundle (ja-JP / en-US) を svelte-i18n に register し、初期 locale を確定するまで
// マウントを遅延する。これにより `$_(key)` が最初のレンダリングから正しい言語を返す
// (= 「英語キーがフラッシュで一瞬見える」を回避)。
// 本番では `init()` 直後に locale が未設定のティックがあり `$_()` が例外 → 白画面になり得るため、
// 辞書ロード完了を `waitLocale()` で待つ。
if (import.meta.env.DEV || import.meta.env.VITE_UN_DESIGN_PREVIEW === "1") {
	const isolated = !("__TAURI_INTERNALS__" in window);
	const { installDevIpcMock } = await import("./dev-ipc-mock");
	installDevIpcMock();
	if (isolated && import.meta.env.VITE_UN_DESIGN_PREVIEW === "1") {
		document.documentElement.dataset.unIsolatedPreview = "1";
	}
}
await setupI18n();
await waitLocale();

const app = mount(App, {
	target: document.getElementById("app") as HTMLElement,
});

export default app;
