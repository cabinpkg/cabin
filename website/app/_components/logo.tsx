import { faHouseChimneyWindow } from "@fortawesome/free-solid-svg-icons";
import { FontAwesomeIcon } from "@fortawesome/react-fontawesome";
import React from "react";

export const Logo = () => (
    <div className="hidden sm:flex gap-1" justify="end">
        <FontAwesomeIcon
            className="text-default-600 dark:text-default-500"
            icon={faHouseChimneyWindow}
            width={18}
        />
        <p className="font-bold text-default-600 dark:text-default-500">
            Cabin
        </p>
    </div>
);
