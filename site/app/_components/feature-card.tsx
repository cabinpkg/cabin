import type { IconDefinition } from "@fortawesome/fontawesome-svg-core";
import { Card, CardBody, CardHeader } from "@heroui/react";
import { GradientIcon } from "./gradient-icon";

interface FeatureCardProps {
    icon: IconDefinition;
    title: string;
    description: string;
    gradientFrom?: string;
    gradientTo?: string;
    borderColor?: string;
    className?: string;
}

export function FeatureCard({
    icon,
    title,
    description,
    gradientFrom = "primary/5",
    gradientTo = "primary/10",
    borderColor = "primary/20",
    className = "",
}: FeatureCardProps) {
    return (
        <Card
            className={`bg-gradient-to-br from-${gradientFrom} to-${gradientTo} border-${borderColor} ${className}`}
        >
            <CardHeader className="pb-4">
                <div className="flex items-center gap-3">
                    <GradientIcon icon={icon} size="lg" />
                    <h3 className="text-xl font-semibold">{title}</h3>
                </div>
            </CardHeader>
            <CardBody className="pt-0">
                <p className="text-default-600">{description}</p>
            </CardBody>
        </Card>
    );
}
