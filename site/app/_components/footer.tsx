import { faGithub } from "@fortawesome/free-brands-svg-icons";
import {
    faBookOpen,
    faHeart,
    faHouseChimneyWindow,
} from "@fortawesome/free-solid-svg-icons";
import { Divider, Link } from "@heroui/react";
import { ExternalLinkButton } from "./external-link-button";
import { GradientIcon } from "./gradient-icon";

export function Footer() {
    return (
        <footer className="bg-gradient-to-t from-default-100/50 to-background border-t border-divider">
            <div className="container mx-auto max-w-7xl px-6 py-16">
                <div className="flex flex-col items-center text-center space-y-8">
                    {/* Logo and tagline */}
                    <div className="flex flex-col items-center space-y-4">
                        <div className="flex items-center gap-3">
                            <GradientIcon
                                icon={faHouseChimneyWindow}
                                size="md"
                            />
                            <h3 className="text-2xl font-bold bg-gradient-to-r from-primary-600 to-secondary-600 bg-clip-text text-transparent">
                                Cabin
                            </h3>
                        </div>
                        <p className="text-default-600 max-w-md">
                            Modern, intuitive, and lightning-fast package
                            manager for C++ developers.
                        </p>
                    </div>

                    {/* Mobile navigation - only shown on small screens */}
                    <div className="flex sm:hidden flex-wrap justify-center gap-3">
                        <ExternalLinkButton
                            href="https://docs.cabinpkg.com"
                            icon={faBookOpen}
                            className="text-default-600 hover:text-primary bg-default-100 hover:bg-primary/10 transition-all"
                        >
                            Documentation
                        </ExternalLinkButton>
                        <ExternalLinkButton
                            href="https://github.com/cabinpkg"
                            icon={faGithub}
                            className="text-default-600 hover:text-primary bg-default-100 hover:bg-primary/10 transition-all"
                        >
                            GitHub
                        </ExternalLinkButton>
                        <ExternalLinkButton
                            href="https://github.com/sponsors/ken-matsui"
                            icon={faHeart}
                            color="danger"
                            className="font-medium shadow-sm hover:shadow-md transition-shadow"
                            iconClassName="animate-pulse"
                        >
                            Sponsor
                        </ExternalLinkButton>
                    </div>

                    {/* Divider */}
                    <div className="w-full max-w-xs">
                        <Divider className="bg-gradient-to-r from-transparent via-default-300 to-transparent" />
                    </div>

                    {/* Copyright and links */}
                    <div className="flex flex-col items-center space-y-3">
                        <p className="text-sm text-default-500">
                            Built by{" "}
                            <Link
                                isExternal
                                href="https://github.com/ken-matsui"
                                className="text-primary hover:text-primary-600 transition-colors font-medium"
                            >
                                Ken Matsui
                            </Link>
                        </p>
                        <p className="text-xs text-default-400">
                            © 2018-{new Date().getFullYear()} Cabin. All rights
                            reserved.
                        </p>
                    </div>
                </div>
            </div>

            {/* Background decoration */}
            <div className="absolute bottom-0 left-1/2 transform -translate-x-1/2 w-[400px] h-[200px] bg-gradient-to-t from-primary/5 to-transparent rounded-full blur-3xl -z-10" />
        </footer>
    );
}
