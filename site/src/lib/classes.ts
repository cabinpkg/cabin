type ClassValue = string | false | null | undefined;

export function joinClasses(...classes: ClassValue[]): string {
    return classes.filter(Boolean).join(" ");
}

export const containerClass = "container-shell";
export const focusRingClass =
    "focus-visible:outline focus-visible:outline-3 focus-visible:outline-offset-3 focus-visible:outline-sky-300/75";
export const mutedTextClass = "text-slate-400";
export const surfaceClass =
    "border border-slate-800 bg-slate-950/70 shadow-xl shadow-black/10";
export const interactiveSurfaceClass =
    "transition hover:-translate-y-0.5 hover:border-sky-400/50 hover:bg-slate-900/90";
export const glassSurfaceClass =
    "border border-slate-700/70 bg-slate-900/70 shadow-2xl shadow-black/30 backdrop-blur";
export const iconSurfaceClass =
    "flex h-10 w-10 shrink-0 items-center justify-center rounded-xl bg-gradient-to-br from-sky-500/20 to-violet-500/20 text-sky-300";

export const buttonBaseClass =
    "inline-flex items-center justify-center gap-2 rounded-full text-sm font-bold transition";
export const buttonVariantClasses = {
    primary:
        "bg-gradient-to-br from-sky-500 to-violet-600 px-5 py-3 text-white shadow-lg shadow-sky-950/40 hover:-translate-y-0.5 hover:shadow-violet-950/40",
    secondary:
        "border border-sky-400/45 px-5 py-3 text-sky-200 hover:-translate-y-0.5 hover:bg-sky-400/10 hover:text-white",
    ghost: "border border-slate-700/80 bg-slate-900/70 px-3 py-2 text-slate-300 hover:border-sky-400/60 hover:bg-sky-400/10 hover:text-white",
} as const;
export const iconButtonClass = "h-10 w-10 px-0 py-0";

export const badgeToneClasses = {
    default: "border-sky-400/24 bg-sky-400/12 text-sky-200",
    success: "border-emerald-400/24 bg-emerald-400/12 text-emerald-200",
    warning: "border-amber-400/24 bg-amber-400/12 text-amber-200",
    muted: "border-slate-700 bg-slate-800/80 text-slate-200",
} as const;

export const sponsorButtonClass =
    "border-rose-400/30 bg-rose-500/10 text-rose-200 hover:border-rose-300/70 hover:bg-rose-500/20";
