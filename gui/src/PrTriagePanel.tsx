import { Modal } from "./ui";
import ScreenshotViewer from "./ScreenshotViewer";
import { PrTriageBody, usePrTriage, type PrTriageProps } from "./PrTriageBody";

/** PR Triage: browse a pull request's review feedback and act on it. Every
 * GitHub write and AI call is shown first and needs an explicit confirmation. */
export default function PrTriagePanel(props: PrTriageProps) {
  const { target, onHandoff, embedded = false } = props;
  const triage = usePrTriage(props);
  const { view, busy, close, currentImage, closeImage, imageIdentity } = triage;
  return (
    <>
    <Modal inline={embedded} label="Review comments" size="xl" dismissable={!embedded && !busy && !currentImage} onClose={close}
      title={view ? `Review comments · ${view.feature_name}` : "Review comments"}
      subtitle="Read and reply to feedback on your pull request: investigate a comment, have an agent prepare a fix, and reply. GitHub writes and AI calls always ask first.">
      <PrTriageBody triage={triage} target={target} onHandoff={onHandoff} embedded={embedded} />
    </Modal>
    {currentImage && <Modal label="PR image" title={currentImage.caption} size="xl" onClose={closeImage}>
      <ScreenshotViewer identity={imageIdentity} caption={currentImage.caption} provenance={[]} index={0} total={1} backLabel="Close image"
        load={() => Promise.resolve(currentImage.image)} onMove={() => {}} onClose={closeImage} />
    </Modal>}
    </>
  );
}
