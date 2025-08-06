import type { IconDefinition } from "@fortawesome/fontawesome-svg-core";
import { Card, CardBody, CardHeader } from "@heroui/react";
import { GradientIcon } from "./gradient-icon";

interface FeatureCardProps {
    icon: IconDefinition;
    title: string;
    description: string;
    colorScheme?: "primary" | "blue" | "purple" | "green" | "orange" | "red";
    className?: string;
}

const cardColorVariants = {
    primary:
        "bg-gradient-to-br from-primary/5 to-secondary/10 border-primary/20",
    blue: "bg-gradient-to-br from-primary/5 to-primary/10 border-primary/20",
    purple: "bg-gradient-to-br from-secondary/5 to-secondary/10 border-secondary/20",
    green: "bg-gradient-to-br from-success/5 to-success/10 border-success/20",
    orange: "bg-gradient-to-br from-warning/5 to-warning/10 border-warning/20",
    red: "bg-gradient-to-br from-danger/5 to-danger/10 border-danger/20",
};

const iconColorMapping = {
    primary: "primary" as const,
    blue: "blue" as const,
    purple: "purple" as const,
    green: "green" as const,
    orange: "orange" as const,
    red: "red" as const,
};

export function FeatureCard({
    icon,
    title,
    description,
    colorScheme = "primary",
    className = "",
}: FeatureCardProps) {
    return (
        <Card className={`${cardColorVariants[colorScheme]} ${className}`}>
            <CardHeader className="pb-4">
                <div className="flex items-center gap-3">
                    <GradientIcon
                        icon={icon}
                        size="lg"
                        colorScheme={iconColorMapping[colorScheme]}
                    />
                    <h3 className="text-xl font-semibold">{title}</h3>
                </div>
            </CardHeader>
            <CardBody className="pt-0">
                <p className="text-default-600">{description}</p>
            </CardBody>
        </Card>
    );
}
