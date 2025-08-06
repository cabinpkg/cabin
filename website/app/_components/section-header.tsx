import type { IconDefinition } from "@fortawesome/fontawesome-svg-core";
import { FontAwesomeIcon } from "@fortawesome/react-fontawesome";

interface SectionHeaderProps {
    icon: IconDefinition;
    title: string;
    iconColor?: string;
    size?: "sm" | "md" | "lg";
    className?: string;
}

const sizeClasses = {
    sm: "text-lg",
    md: "text-xl",
    lg: "text-2xl",
};

export function SectionHeader({
    icon,
    title,
    iconColor = "text-primary",
    size = "md",
    className = "",
}: SectionHeaderProps) {
    return (
        <div className={`flex items-center gap-2 ${className}`}>
            <FontAwesomeIcon icon={icon} className={`${iconColor} text-lg`} />
            <h2 className={`${sizeClasses[size]} font-semibold`}>{title}</h2>
        </div>
    );
}
