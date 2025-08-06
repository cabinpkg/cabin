import type { IconDefinition } from "@fortawesome/fontawesome-svg-core";
import { FontAwesomeIcon } from "@fortawesome/react-fontawesome";
import { Chip } from "@heroui/react";
import type { ReactNode } from "react";

interface StatItemProps {
    icon: IconDefinition;
    label: string;
    value: ReactNode;
    className?: string;
}

export function StatItem({
    icon,
    label,
    value,
    className = "",
}: StatItemProps) {
    return (
        <div className={`flex items-center justify-between ${className}`}>
            <div className="flex items-center gap-2">
                <FontAwesomeIcon icon={icon} className="text-default-500" />
                <span className="text-default-600">{label}</span>
            </div>
            {typeof value === "string" || typeof value === "number" ? (
                <Chip size="sm" variant="flat">
                    {value}
                </Chip>
            ) : (
                value
            )}
        </div>
    );
}
