import type { IconDefinition } from "@fortawesome/fontawesome-svg-core";
import { FontAwesomeIcon } from "@fortawesome/react-fontawesome";

interface GradientIconProps {
    icon: IconDefinition;
    size?: "sm" | "md" | "lg" | "xl";
    className?: string;
    iconClassName?: string;
}

const sizeClasses = {
    sm: "w-8 h-8",
    md: "w-10 h-10",
    lg: "w-12 h-12",
    xl: "w-16 h-16",
};

const iconSizeClasses = {
    sm: "text-sm",
    md: "text-sm",
    lg: "text-lg",
    xl: "text-xl",
};

export function GradientIcon({
    icon,
    size = "md",
    className = "",
    iconClassName = "",
}: GradientIconProps) {
    return (
        <div
            className={`flex items-center justify-center ${sizeClasses[size]} bg-gradient-to-br from-primary-500 to-secondary-500 rounded-lg shadow-sm ${className}`}
        >
            <FontAwesomeIcon
                icon={icon}
                className={`text-white ${iconSizeClasses[size]} ${iconClassName}`}
            />
        </div>
    );
}
