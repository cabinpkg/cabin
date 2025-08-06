import type { IconDefinition } from "@fortawesome/fontawesome-svg-core";
import { FontAwesomeIcon } from "@fortawesome/react-fontawesome";
import { Button, Link } from "@heroui/react";

interface ExternalLinkButtonProps {
    href: string;
    icon: IconDefinition;
    children: React.ReactNode;
    variant?:
        | "light"
        | "flat"
        | "bordered"
        | "ghost"
        | "shadow"
        | "solid"
        | "faded";
    color?:
        | "default"
        | "primary"
        | "secondary"
        | "success"
        | "warning"
        | "danger";
    size?: "sm" | "md" | "lg";
    className?: string;
    isIconOnly?: boolean;
    fullWidth?: boolean;
    iconClassName?: string;
    animate?: boolean;
}

export function ExternalLinkButton({
    href,
    icon,
    children,
    variant = "flat",
    color = "default",
    size = "sm",
    className = "",
    isIconOnly = false,
    fullWidth = false,
    iconClassName = "",
    animate = false,
}: ExternalLinkButtonProps) {
    const buttonClass = `${fullWidth ? "w-full justify-start" : ""} ${className}`;
    const iconClasses = `text-sm ${animate ? "animate-pulse" : ""} ${iconClassName}`;

    return (
        <Button
            as={Link}
            href={href}
            isExternal
            variant={variant}
            color={color}
            size={size}
            className={buttonClass}
            isIconOnly={isIconOnly}
            startContent={
                !isIconOnly ? (
                    <FontAwesomeIcon icon={icon} className={iconClasses} />
                ) : undefined
            }
        >
            {isIconOnly ? (
                <FontAwesomeIcon icon={icon} className={iconClasses} />
            ) : (
                children
            )}
        </Button>
    );
}
