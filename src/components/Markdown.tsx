import { micromark } from "micromark";

export interface MarkdownProps {
  content: string;
  class?: string;
}

export default function Markdown(props: MarkdownProps) {
  return (
    <div
      class={props.class}
      innerHTML={micromark(props.content)}
    />
  );
}
