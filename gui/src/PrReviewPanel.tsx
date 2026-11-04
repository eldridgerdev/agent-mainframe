import { Modal } from "./ui";
import { PrReviewBody, usePrReview, type EmbeddedReview } from "./PrReviewBody";

export type { EmbeddedReview };

export default function PrReviewPanel({ projectId, onClose, embedded }: { projectId: string; onClose: () => void; embedded?: EmbeddedReview }) {
  const review = usePrReview({ projectId, onClose, embedded });
  const { view, locked, leave } = review;
  const body = <PrReviewBody review={review} embedded={embedded} />;
  if (embedded) return <section className="pr-review-embedded" aria-label="PR Files review">{body}</section>;
  return <Modal label="Write a review" title={`Write a review${view ? ` · ${view.project_name}` : ""}`} size="xl"
    subtitle="Comment on someone else's pull request, then preview and post your review to GitHub."
    dismissable={!locked} onClose={() => leave("close")}>{body}</Modal>;
}
