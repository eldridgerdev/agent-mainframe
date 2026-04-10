type BadgeProps = {
  label: string;
  tone?: "info" | "warn";
};

export function Badge({ label, tone = "info" }: BadgeProps) {
  return (
    <span className={`badge badge-${tone}`}>
      <em>{label}</em>
    </span>
  );
}
