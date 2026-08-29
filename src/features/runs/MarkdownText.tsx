import DOMPurify from "dompurify";
import hljs from "highlight.js/lib/core";
import bash from "highlight.js/lib/languages/bash";
import css from "highlight.js/lib/languages/css";
import javascript from "highlight.js/lib/languages/javascript";
import json from "highlight.js/lib/languages/json";
import markdownLanguage from "highlight.js/lib/languages/markdown";
import python from "highlight.js/lib/languages/python";
import rust from "highlight.js/lib/languages/rust";
import typescript from "highlight.js/lib/languages/typescript";
import xml from "highlight.js/lib/languages/xml";
import { Marked } from "marked";
import { markedHighlight } from "marked-highlight";

for (const [name, language] of [
  ["bash", bash],
  ["sh", bash],
  ["shell", bash],
  ["css", css],
  ["javascript", javascript],
  ["js", javascript],
  ["json", json],
  ["markdown", markdownLanguage],
  ["md", markdownLanguage],
  ["python", python],
  ["py", python],
  ["rust", rust],
  ["rs", rust],
  ["typescript", typescript],
  ["ts", typescript],
  ["tsx", typescript],
  ["html", xml],
  ["xml", xml],
] as const) {
  if (!hljs.getLanguage(name)) hljs.registerLanguage(name, language);
}

const markdown = new Marked(
  markedHighlight({
    langPrefix: "hljs language-",
    highlight(code, language) {
      const normalized = language.toLowerCase();
      if (!hljs.getLanguage(normalized)) return escapeHtml(code);
      return hljs.highlight(code, { language: normalized }).value;
    },
  }),
);

markdown.setOptions({ gfm: true, breaks: true });
markdown.use({
  renderer: {
    html({ text }) {
      return escapeHtml(text);
    },
  },
});

function escapeHtml(value: string): string {
  return value
    .replaceAll("&", "&amp;")
    .replaceAll("<", "&lt;")
    .replaceAll(">", "&gt;")
    .replaceAll('"', "&quot;")
    .replaceAll("'", "&#39;");
}

function renderMarkdown(text: string): string {
  const html = markdown.parse(text, { async: false });
  return DOMPurify.sanitize(typeof html === "string" ? html : text, {
    ALLOWED_TAGS: [
      "p",
      "br",
      "hr",
      "strong",
      "em",
      "del",
      "blockquote",
      "ul",
      "ol",
      "li",
      "h1",
      "h2",
      "h3",
      "h4",
      "pre",
      "code",
      "span",
      "table",
      "thead",
      "tbody",
      "tr",
      "th",
      "td",
    ],
    ALLOWED_ATTR: ["class"],
  });
}

export function MarkdownText(props: { text: string; class?: string }) {
  return (
    <div
      class={`markdown-body${props.class ? ` ${props.class}` : ""}`}
      innerHTML={renderMarkdown(props.text)}
    />
  );
}
