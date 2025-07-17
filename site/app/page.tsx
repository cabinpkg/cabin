import { faArrowRight, faDownload } from "@fortawesome/free-solid-svg-icons";
import { FontAwesomeIcon } from "@fortawesome/react-fontawesome";
import { Button, Link } from "@nextui-org/react";
import type { Metadata } from "next";
import Image from "next/image";

export const revalidate = 86400; // 1 day

export const metadata: Metadata = {
    title: "Cabin - Intuitive and fast C++ package manager and build system",
};

export default function Home() {
    return (
        <main className="container mx-auto max-w-7xl px-6 flex-grow">
            <section className="flex flex-col items-center justify-center">
                <section className="flex relative overflow-hidden lg:overflow-visible w-full flex-nowrap justify-between items-center h-[calc(100vh_-_64px)] 2xl:h-[calc(84vh_-_64px)]">
                    <div className="grid grid-cols-1 lg:grid-cols-2 gap-4">
                        <div className="text-center leading-8 md:leading-10 md:text-left">
                            <div className="inline-block">
                                <h1 className="font-bold text-3xl lg:text-6xl">
                                    <span className="from-[#51DEEC] to-[#3023AE] bg-clip-text text-transparent bg-gradient-to-b">
                                        Effortlessly
                                    </span>{" "}
                                    build and share your C++ packages.
                                </h1>
                            </div>
                            <h2 className="my-4 text-lg lg:text-xl font-normal text-default-500">
                                Intuitive and fast package manager and build
                                system.
                            </h2>
                            <div>
                                <Button
                                    isExternal
                                    as={Link}
                                    className="w-full md:w-auto"
                                    color="primary"
                                    href="https://docs.cabinpkg.com/installation/"
                                    radius="full"
                                    size="lg"
                                    startContent={
                                        <FontAwesomeIcon
                                            icon={faDownload}
                                            width={15}
                                        />
                                    }
                                >
                                    Install Cabin
                                </Button>
                                <Button
                                    isExternal
                                    as={Link}
                                    className="mx-0 my-4 md:mx-4 md:my-0 w-full md:w-auto"
                                    href="https://docs.cabinpkg.com"
                                    radius="full"
                                    size="lg"
                                    endContent={
                                        <FontAwesomeIcon
                                            icon={faArrowRight}
                                            width={15}
                                        />
                                    }
                                >
                                    Getting Started
                                </Button>
                            </div>
                        </div>
                        <div className="relative">
                            <Image
                                src="https://vhs.charm.sh/vhs-12NaAvXqgDiV647TA2C356.gif"
                                width={800}
                                height={450}
                                alt="Demo of Cabin"
                            />
                        </div>
                    </div>
                </section>
            </section>
        </main>
    );
}
