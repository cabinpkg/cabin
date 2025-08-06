import { faGithub } from "@fortawesome/free-brands-svg-icons";
import { faBookOpen, faHeart } from "@fortawesome/free-solid-svg-icons";
import { FontAwesomeIcon } from "@fortawesome/react-fontawesome";
import { Button, Divider, Link } from "@heroui/react";

export function Footer() {
    return (
        <footer className="container mx-auto max-w-7xl pb-12 px-12">
            <div className="flex flex-col justify-center items-center gap-4">
                {/* Mobile navigation - only shown on small screens */}
                <div className="flex sm:hidden gap-4">
                    <Button
                        as={Link}
                        href="https://docs.cabinpkg.com"
                        isExternal
                        variant="light"
                        size="sm"
                        className="text-default-600 hover:text-primary transition-colors"
                        startContent={
                            <FontAwesomeIcon
                                icon={faBookOpen}
                                className="text-sm"
                            />
                        }
                    >
                        Docs
                    </Button>
                    <Button
                        as={Link}
                        href="https://github.com/cabinpkg"
                        isExternal
                        variant="light"
                        size="sm"
                        className="text-default-600 hover:text-primary transition-colors"
                        startContent={
                            <FontAwesomeIcon
                                icon={faGithub}
                                className="text-sm"
                            />
                        }
                    >
                        GitHub
                    </Button>
                    <Button
                        as={Link}
                        href="https://github.com/sponsors/ken-matsui"
                        isExternal
                        color="danger"
                        variant="flat"
                        size="sm"
                        className="font-medium shadow-sm hover:shadow-md transition-shadow"
                        startContent={
                            <FontAwesomeIcon
                                icon={faHeart}
                                className="text-sm animate-pulse"
                            />
                        }
                    >
                        Sponsor
                    </Button>
                </div>

                {/* Divider for mobile */}
                <Divider className="sm:hidden" />

                {/* Copyright */}
                <p className="text-sm text-default-400 text-center">
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
