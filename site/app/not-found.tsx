import type { Metadata } from "next";

export const metadata: Metadata = {
    title: "404: Not Found",
};

export default function Custom404(): React.ReactElement {
    return (
        <div className="flex flex-col items-center justify-center h-screen">
            <h1 className="font-bold m-4 text-6xl">404</h1>
            This page could not be found.
        </div>
    );
}
