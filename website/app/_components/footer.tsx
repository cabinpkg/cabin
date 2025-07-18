import { Link } from "@heroui/react";

export function Footer() {
    return (
        <footer className="container mx-auto max-w-7xl pb-12 px-12">
            <div className="flex flex-col justify-center items-center gap-1">
                <p className="text-sm text-default-400">
                    © 2018-{new Date().getFullYear()}{" "}
                    <Link
                        isExternal
                        aria-label="Ken Matsui"
                        className="text-sm text-default-400"
                        href="https://github.com/ken-matsui"
                    >
                        Ken Matsui
                    </Link>
                </p>
            </div>
        </footer>
    );
}
