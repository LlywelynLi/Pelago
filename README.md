# Pelago

用 Rust 写的简易网页浏览器。打开一个 URL，或直接渲染一段 HTML。

## 结构

```
src/
  main.rs          # 入口与命令行
  app/             # 窗口、地址栏等 UI
  engine/          # DOM / HTML / CSS / 布局 / 绘制 / 图片 / JS
  net/             # HTTP 拉取与 URL 工具
```

## 用法

```
cargo run
cargo run -- <url>
cargo run -- --html "<html>"
```

不带参数时会打开欢迎页，可在地址栏输入网址继续浏览。

## JavaScript

内嵌 [Boa](https://boajs.dev/) 引擎，覆盖浏览器常用宿主能力：

- **DOM**：`createElement` / `createTextNode` / `createDocumentFragment`、`appendChild` / `insertBefore` / `removeChild` / `cloneNode`、属性读写、`querySelector`、父子节点关系
- **事件**：`addEventListener` / `dispatchEvent`，页面脚本跑完后会触发 `DOMContentLoaded` 和 `load`
- **模块加载**：内联与外链 `<script>`、动态插入的 `script`、AMD `define` / `require`（可拉取 URL）
- **其他**：`location`、`navigator`、`console`、`Image`、`XMLHttpRequest`、`fetch`、`localStorage`、最小 `$`

样式会下载 `<link rel="stylesheet">` 和 `@import`。选择器支持标签、class、id、后代和子代。布局支持 `flex`、`grid`、`float` 和 `position: absolute`。

真实浏览器级事件循环、完整 CSSOM 回流和全部 Web API 仍未实现；复杂站点可能仍有脚本失败，会汇总提示。

## 构建

需要 [Rust](https://www.rust-lang.org/)（edition 2024）。

```
cargo build --release
```
