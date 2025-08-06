import { faHouseChimneyWindow } from "@fortawesome/free-solid-svg-icons";
import { FontAwesomeIcon } from "@fortawesome/react-fontawesome";

export const Logo = () => (
    <div className="flex items-center gap-2 hover:opacity-80 transition-opacity">
        <div className="flex items-center justify-center w-8 h-8 bg-gradient-to-br from-primary-500 to-secondary-500 rounded-lg shadow-sm">
            <FontAwesomeIcon
                icon={faHouseChimneyWindow}
                className="text-white text-sm"
            />
        </div>
        <div className="hidden sm:block">
            <h1 className="font-bold text-lg bg-gradient-to-r from-primary-600 to-secondary-600 bg-clip-text text-transparent">
                Cabin
            </h1>
        </div>
    </div>
);
