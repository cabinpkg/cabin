// This code is based on the original code found at Next UI.
// https://github.com/nextui-org/nextui/blob/132efbcb6d3828ec7aab2cb2d0c79db3868dee92/apps/docs/components/code-window/window-actions.tsx
// Modifications have been made to fit the specific needs of this project.
/*
MIT License

Copyright (c) 2020 Next UI

Permission is hereby granted, free of charge, to any person obtaining a copy
of this software and associated documentation files (the "Software"), to deal
in the Software without restriction, including without limitation the rights
to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
copies of the Software, and to permit persons to whom the Software is
furnished to do so, subject to the following conditions:

The above copyright notice and this permission notice shall be included in all
copies or substantial portions of the Software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE
SOFTWARE.
*/

import { clsx } from "@nextui-org/shared-utils";
import type React from "react";
import { tv } from "tailwind-variants";

export type WindowActionsProps = {
    title?: string;
    className?: string;
};

const windowIconStyles = tv({
    base: "w-3 h-3 rounded-full",
    variants: {
        color: {
            red: "bg-red-500",
            yellow: "bg-yellow-500",
            green: "bg-green-500",
        },
    },
});

export const WindowActions: React.FC<WindowActionsProps> = ({
    title,
    className,
    ...props
}) => {
    return (
        <div
            className={clsx(
                "flex items-center sticky top-0 left-0 px-4 z-10 justify-between h-8 bg-code-background w-full",
                className,
            )}
            {...props}
        >
            <div className="flex items-center gap-2 basis-1/3">
                <div className={windowIconStyles({ color: "red" })} />
                <div className={windowIconStyles({ color: "yellow" })} />
                <div className={windowIconStyles({ color: "green" })} />
            </div>
            <div className="flex basis-1/3 h-full justify-center items-center">
                {title && <p className="text-white/30 text-sm">{title}</p>}
            </div>
            <div className="flex basis-1/3" />
        </div>
    );
};
