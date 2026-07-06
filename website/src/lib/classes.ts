type ClassValue = string | false | null | undefined;

export function joinClasses(...classes: ClassValue[]): string {
    return classes.filter(Boolean).join(" ");
}

export const focusRingClass =
    "focus-visible:outline-2 focus-visible:outline-offset-3 focus-visible:outline-steel";
export const surfaceClass = "border border-line bg-night-raised";
export const interactiveSurfaceClass =
    "transition-colors hover:border-steel/60 hover:bg-night-lifted";

export const buttonBaseClass =
    "inline-flex items-center justify-center gap-2 rounded-sm text-sm font-semibold transition-colors";
export const buttonVariantClasses = {
    primary: "bg-steel px-5 py-3 text-steel-ink hover:bg-steel-bright",
    secondary:
        "border border-line-strong px-5 py-3 text-ink hover:border-steel hover:text-steel-bright",
    ghost: "border border-transparent px-3 py-2 text-ink-muted hover:border-line hover:bg-night-lifted hover:text-ink",
} as const;
export const iconButtonClass = "h-10 w-10 px-0 py-0";

export const badgeToneClasses = {
    default: "border-steel/35 bg-steel/10 text-steel-bright",
    success: "border-pine/35 bg-pine/10 text-pine",
    warning: "border-steel/35 bg-steel/10 text-steel-bright",
    muted: "border-line-strong bg-night-lifted text-ink-muted",
} as const;
