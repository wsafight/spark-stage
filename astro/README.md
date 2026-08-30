# SparkStage Astro 文档站

这是 SparkStage 仓库内的 Astro 5 静态文档站。站点只负责展示，文档源文件仍然是仓库根目录的 `docs/`，避免 Markdown 在两个目录分别维护。

## 本地运行

```bash
cd astro
npm install
npm run dev
```

生产构建：

```bash
npm run build
```

构建输出位于 `astro/dist/`，不会提交到 Git。文档链接和占位符检查仍由根目录的 `scripts/check-docs.sh` 负责。

站点按 GitHub Pages 项目路径配置，线上地址是 [https://wsafight.github.io/spark-stage/](https://wsafight.github.io/spark-stage/)。本地开发时同样挂在 `/spark-stage/` 下，例如 `http://localhost:4321/spark-stage/`。
