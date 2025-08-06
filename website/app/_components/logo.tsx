import { faHouseChimneyWindow } from "@fortawesome/free-solid-svg-icons";
import { GradientIcon } from "./gradient-icon";

export const Logo = () => (
    <div className="flex items-center gap-2 hover:opacity-80 transition-opacity">
        <GradientIcon icon={faHouseChimneyWindow} size="sm" />
        <div className="hidden sm:block">
            <h1 className="font-bold text-lg bg-gradient-to-r from-primary-600 to-secondary-600 bg-clip-text text-transparent">
                Cabin
            </h1>
        </div>
    </div>
);
