import {
    faArrowRight,
    faBolt,
    faBox,
    faCode,
    faCog,
    faDownload,
    faHouseChimneyWindow,
    faRocket,
} from "@fortawesome/free-solid-svg-icons";
import { FontAwesomeIcon } from "@fortawesome/react-fontawesome";
import { Button, Card, CardBody, CardHeader, Link } from "@heroui/react";
import type { Metadata } from "next";
import Image from "next/image";

export const revalidate = 86400; // 1 day

export const metadata: Metadata = {
    title: "Cabin - Intuitive and fast C++ package manager and build system",
};

export default function Home() {
    return (
        <main className="min-h-screen">
            {/* Hero Section */}
            <section className="relative overflow-hidden bg-gradient-to-br from-background via-background to-primary/5">
                <div className="container mx-auto max-w-7xl px-6 py-16 md:py-24">
                    <div className="grid grid-cols-1 lg:grid-cols-2 gap-12 lg:gap-16 items-center">
                        {/* Content */}
                        <div className="text-center lg:text-left space-y-6">
                            <div className="space-y-4">
                                <h1 className="text-4xl md:text-5xl lg:text-6xl font-bold leading-tight">
                                    <span className="bg-gradient-to-r from-primary-600 to-secondary-600 bg-clip-text text-transparent">
                                        Effortlessly
                                    </span>{" "}
                                    build and share your{" "}
                                    <span className="bg-gradient-to-r from-secondary-600 to-primary-600 bg-clip-text text-transparent">
                                        C++ packages
                                    </span>
                                </h1>
                                <p className="text-xl md:text-2xl text-default-600 max-w-2xl">
                                    Modern, intuitive, and lightning-fast
                                    package manager and build system for C++
                                    developers.
                                </p>
                            </div>

                            {/* CTA Buttons */}
                            <div className="flex flex-col sm:flex-row gap-4 justify-center lg:justify-start">
                                <Button
                                    as={Link}
                                    href="https://docs.cabinpkg.com/installation/"
                                    isExternal
                                    color="primary"
                                    size="lg"
                                    className="font-semibold shadow-lg hover:shadow-xl transition-shadow"
                                    startContent={
                                        <FontAwesomeIcon
                                            icon={faDownload}
                                            className="text-sm"
                                        />
                                    }
                                >
                                    Install Cabin
                                </Button>
                                <Button
                                    as={Link}
                                    href="https://docs.cabinpkg.com"
                                    isExternal
                                    variant="bordered"
                                    size="lg"
                                    className="font-semibold border-primary text-primary hover:bg-primary hover:text-white transition-colors"
                                    endContent={
                                        <FontAwesomeIcon
                                            icon={faArrowRight}
                                            className="text-sm"
                                        />
                                    }
                                >
                                    Documentation
                                </Button>
                            </div>
                        </div>

                        {/* Demo */}
                        <div className="relative">
                            <div className="relative bg-gradient-to-br from-default-100 to-default-200 rounded-2xl p-6 shadow-2xl">
                                <Image
                                    src="https://vhs.charm.sh/vhs-12NaAvXqgDiV647TA2C356.gif"
                                    width={800}
                                    height={450}
                                    alt="Demo of Cabin package manager"
                                    priority
                                    unoptimized
                                    className="rounded-xl shadow-lg w-full h-auto"
                                />
                            </div>
                            {/* Floating badge */}
                            <div className="absolute -top-4 -right-4 bg-success rounded-full p-3 shadow-lg">
                                <FontAwesomeIcon
                                    icon={faRocket}
                                    className="text-white text-lg"
                                />
                            </div>
                        </div>
                    </div>
                </div>

                {/* Background decoration */}
                <div className="absolute top-1/2 left-1/2 transform -translate-x-1/2 -translate-y-1/2 w-[800px] h-[800px] bg-gradient-to-r from-primary/10 to-secondary/10 rounded-full blur-3xl -z-10" />
            </section>

            {/* Features Section */}
            <section className="py-16 md:py-24">
                <div className="container mx-auto max-w-7xl px-6">
                    <div className="text-center mb-16">
                        <h2 className="text-3xl md:text-4xl font-bold mb-4">
                            Why Choose{" "}
                            <span className="bg-gradient-to-r from-primary-600 to-secondary-600 bg-clip-text text-transparent">
                                Cabin
                            </span>
                            ?
                        </h2>
                        <p className="text-xl text-default-600 max-w-3xl mx-auto">
                            Built for modern C++ development with developer
                            experience as the top priority.
                        </p>
                    </div>

                    <div className="grid grid-cols-1 md:grid-cols-2 lg:grid-cols-3 gap-8">
                        {/* Feature 1 */}
                        <Card className="bg-gradient-to-br from-primary/5 to-primary/10 border-primary/20">
                            <CardHeader className="pb-4">
                                <div className="flex items-center gap-3">
                                    <div className="flex items-center justify-center w-12 h-12 bg-gradient-to-br from-primary-500 to-primary-600 rounded-lg">
                                        <FontAwesomeIcon
                                            icon={faBolt}
                                            className="text-white text-lg"
                                        />
                                    </div>
                                    <h3 className="text-xl font-semibold">
                                        Lightning Fast
                                    </h3>
                                </div>
                            </CardHeader>
                            <CardBody className="pt-0">
                                <p className="text-default-600">
                                    Optimized build system with parallel
                                    processing and smart caching for blazing
                                    fast builds.
                                </p>
                            </CardBody>
                        </Card>

                        {/* Feature 2 */}
                        <Card className="bg-gradient-to-br from-secondary/5 to-secondary/10 border-secondary/20">
                            <CardHeader className="pb-4">
                                <div className="flex items-center gap-3">
                                    <div className="flex items-center justify-center w-12 h-12 bg-gradient-to-br from-secondary-500 to-secondary-600 rounded-lg">
                                        <FontAwesomeIcon
                                            icon={faCode}
                                            className="text-white text-lg"
                                        />
                                    </div>
                                    <h3 className="text-xl font-semibold">
                                        Developer Friendly
                                    </h3>
                                </div>
                            </CardHeader>
                            <CardBody className="pt-0">
                                <p className="text-default-600">
                                    Intuitive CLI with helpful error messages
                                    and comprehensive documentation.
                                </p>
                            </CardBody>
                        </Card>

                        {/* Feature 3 */}
                        <Card className="bg-gradient-to-br from-success/5 to-success/10 border-success/20">
                            <CardHeader className="pb-4">
                                <div className="flex items-center gap-3">
                                    <div className="flex items-center justify-center w-12 h-12 bg-gradient-to-br from-success-500 to-success-600 rounded-lg">
                                        <FontAwesomeIcon
                                            icon={faBox}
                                            className="text-white text-lg"
                                        />
                                    </div>
                                    <h3 className="text-xl font-semibold">
                                        Rich Ecosystem
                                    </h3>
                                </div>
                            </CardHeader>
                            <CardBody className="pt-0">
                                <p className="text-default-600">
                                    Growing ecosystem of C++ packages with
                                    seamless dependency management.
                                </p>
                            </CardBody>
                        </Card>

                        {/* Feature 4 */}
                        <Card className="bg-gradient-to-br from-warning/5 to-warning/10 border-warning/20">
                            <CardHeader className="pb-4">
                                <div className="flex items-center gap-3">
                                    <div className="flex items-center justify-center w-12 h-12 bg-gradient-to-br from-warning-500 to-warning-600 rounded-lg">
                                        <FontAwesomeIcon
                                            icon={faCog}
                                            className="text-white text-lg"
                                        />
                                    </div>
                                    <h3 className="text-xl font-semibold">
                                        Zero Configuration
                                    </h3>
                                </div>
                            </CardHeader>
                            <CardBody className="pt-0">
                                <p className="text-default-600">
                                    Works out of the box with sensible defaults.
                                    No complex setup required.
                                </p>
                            </CardBody>
                        </Card>

                        {/* Feature 5 */}
                        <Card className="bg-gradient-to-br from-danger/5 to-danger/10 border-danger/20">
                            <CardHeader className="pb-4">
                                <div className="flex items-center gap-3">
                                    <div className="flex items-center justify-center w-12 h-12 bg-gradient-to-br from-danger-500 to-danger-600 rounded-lg">
                                        <FontAwesomeIcon
                                            icon={faRocket}
                                            className="text-white text-lg"
                                        />
                                    </div>
                                    <h3 className="text-xl font-semibold">
                                        Modern Standards
                                    </h3>
                                </div>
                            </CardHeader>
                            <CardBody className="pt-0">
                                <p className="text-default-600">
                                    Built with modern C++ standards and best
                                    practices in mind.
                                </p>
                            </CardBody>
                        </Card>

                        {/* Feature 6 */}
                        <Card className="bg-gradient-to-br from-primary/5 to-secondary/10 border-primary/20">
                            <CardHeader className="pb-4">
                                <div className="flex items-center gap-3">
                                    <div className="flex items-center justify-center w-12 h-12 bg-gradient-to-br from-primary-500 to-secondary-500 rounded-lg">
                                        <FontAwesomeIcon
                                            icon={faHouseChimneyWindow}
                                            className="text-white text-lg"
                                        />
                                    </div>
                                    <h3 className="text-xl font-semibold">
                                        Cross Platform
                                    </h3>
                                </div>
                            </CardHeader>
                            <CardBody className="pt-0">
                                <p className="text-default-600">
                                    Works seamlessly on macOS and Linux with
                                    consistent behavior.
                                </p>
                            </CardBody>
                        </Card>
                    </div>
                </div>
            </section>

            {/* CTA Section */}
            <section className="py-16 md:py-24 bg-gradient-to-r from-primary/10 to-secondary/10">
                <div className="container mx-auto max-w-4xl px-6 text-center">
                    <h2 className="text-3xl md:text-4xl font-bold mb-4">
                        Ready to build something amazing?
                    </h2>
                    <p className="text-xl text-default-600 mb-8">
                        Join the growing community of developers building better
                        C++ applications with Cabin.
                    </p>
                    <div className="flex flex-col sm:flex-row gap-4 justify-center">
                        <Button
                            as={Link}
                            href="https://docs.cabinpkg.com/installation/"
                            isExternal
                            color="primary"
                            size="lg"
                            className="font-semibold shadow-lg hover:shadow-xl transition-shadow"
                            startContent={
                                <FontAwesomeIcon
                                    icon={faDownload}
                                    className="text-sm"
                                />
                            }
                        >
                            Get Started Now
                        </Button>
                        <Button
                            as={Link}
                            href="/search"
                            variant="bordered"
                            size="lg"
                            className="font-semibold border-primary text-primary hover:bg-primary hover:text-white transition-colors"
                            startContent={
                                <FontAwesomeIcon
                                    icon={faBox}
                                    className="text-sm"
                                />
                            }
                        >
                            Browse Packages
                        </Button>
                    </div>
                </div>
            </section>
        </main>
    );
}
