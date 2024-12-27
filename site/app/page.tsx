import { faArrowRight, faDownload } from "@fortawesome/free-solid-svg-icons";
import { FontAwesomeIcon } from "@fortawesome/react-fontawesome";
import { Code } from "@nextui-org/react";
import { Button, Link } from "@nextui-org/react";
import type { Metadata } from "next";

import { WindowActions } from "./_components/window-actions";

export const revalidate = 86400; // 1 day

export const metadata: Metadata = {
    title: "Cabin - Intuitive and fast C++ package manager and build system",
};

const green = "text-[#ABCF76]";
const brightGreen = "text-[#C3E88D] font-bold";

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
                                    href="https://github.com/cabinpkg/cabin#installation"
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
                            <WindowActions title="Terminal" />
                            <Code className="text-md max-[600px]:text-xs shadow-medium p-4 bg-[#0D0B0B]">
                                $ <span className={green}>cabin</span> new
                                hello_world
                                <br />
                                &nbsp;&nbsp;&nbsp;
                                <span className={brightGreen}>Created</span>{" "}
                                binary (application) `hello_world` package
                                <br />
                                <br />$ <span className={green}>cd</span>{" "}
                                hello_world
                                <br />
                                <br />$ <span className={green}>cabin</span> run
                                <br />
                                &nbsp;
                                <span className={brightGreen}>Compiling</span>{" "}
                                src/main.cc
                                <br />
                                &nbsp;&nbsp;&nbsp;
                                <span className={brightGreen}>Linking</span>{" "}
                                hello_world
                                <br />
                                &nbsp;&nbsp;
                                <span className={brightGreen}>Finished</span>{" "}
                                debug target(s) in 1.28333s
                                <br />
                                &nbsp;&nbsp;&nbsp;
                                <span className={brightGreen}>Running</span>{" "}
                                cabin-out/debug/hello_world
                                <br />
                                Hello, world!
                            </Code>
                        </div>
                    </div>
                </section>
            </section>
        </main>
    );
}
