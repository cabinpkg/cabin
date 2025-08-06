import type { IconDefinition } from "@fortawesome/fontawesome-svg-core";
import { FontAwesomeIcon } from "@fortawesome/react-fontawesome";

interface GradientIconProps {
    icon: IconDefinition;
    size?: "sm" | "md" | "lg" | "xl";
    colorScheme?: "primary" | "blue" | "purple" | "green" | "orange" | "red";
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

const colorVariants = {
    primary: "bg-gradient-to-br from-primary-500 to-secondary-500",
    blue: "bg-gradient-to-br from-primary-500 to-primary-600",
    purple: "bg-gradient-to-br from-secondary-500 to-secondary-600",
    green: "bg-gradient-to-br from-success-500 to-success-600",
    orange: "bg-gradient-to-br from-warning-500 to-warning-600",
    red: "bg-gradient-to-br from-danger-500 to-danger-600",
};

export function GradientIcon({
    icon,
    size = "md",
    colorScheme = "primary",
    className = "",
    iconClassName = "",
}: GradientIconProps) {
    return (
        <div
            className={`flex items-center justify-center ${sizeClasses[size]} ${colorVariants[colorScheme]} rounded-lg shadow-sm ${className}`}
        >
            <FontAwesomeIcon
                icon={icon}
                className={`text-white ${iconSizeClasses[size]} ${iconClassName}`}
            />
        </div>
    );
}
