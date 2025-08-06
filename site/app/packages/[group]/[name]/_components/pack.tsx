import {
    faBookOpen,
    faBox,
    faCalendarAlt,
    faCode,
    faCodeBranch,
    faCube,
    faDownload,
    faGavel,
    faHome,
    faTag,
} from "@fortawesome/free-solid-svg-icons";
import { FontAwesomeIcon } from "@fortawesome/react-fontawesome";
import {
    Button,
    Card,
    CardBody,
    CardHeader,
    Chip,
    Code,
    Link,
} from "@heroui/react";
import ReactMarkdown from "react-markdown";
import { format } from "timeago.js";
import type { GetPackageByNameAndVersionQuery } from "~/graphql";

export function Pack({
    pack,
    numVersion,
}: {
    pack: GetPackageByNameAndVersionQuery["packages"][0];
    numVersion: number;
}) {
    return (
        <div className="container mx-auto max-w-7xl px-6 py-8">
            {/* Hero Section */}
            <div className="mb-8">
                <div className="flex flex-col gap-4">
                    <div className="flex flex-col sm:flex-row sm:items-center gap-4">
                        <div className="flex items-center gap-3">
                            <div className="flex items-center justify-center w-12 h-12 bg-gradient-to-br from-primary-500 to-secondary-500 rounded-lg">
                                <FontAwesomeIcon
                                    icon={faCube}
                                    className="text-white text-lg"
                                />
                            </div>
                            <div>
                                <h1 className="text-3xl md:text-4xl font-bold">
                                    {pack.name}
                                </h1>
                                <div className="flex items-center gap-2 mt-1">
                                    <FontAwesomeIcon
                                        icon={faTag}
                                        className="text-default-500 text-sm"
                                    />
                                    <span className="text-lg text-default-500">
                                        v{pack.version}
                                    </span>
                                </div>
                            </div>
                        </div>
                        <div className="sm:ml-auto">
                            <Chip
                                color="primary"
                                variant="flat"
                                startContent={
                                    <FontAwesomeIcon
                                        icon={faCode}
                                        className="text-sm"
                                    />
                                }
                            >
                                C++{pack.edition.toString().slice(-2)}
                            </Chip>
                        </div>
                    </div>
                    {pack.description && (
                        <p className="text-xl text-default-600 max-w-3xl">
                            {pack.description}
                        </p>
                    )}
                </div>
            </div>

            <div className="grid grid-cols-1 lg:grid-cols-3 gap-6">
                {/* Main Content */}
                <div className="lg:col-span-2 space-y-6">
                    {/* Installation */}
                    <Card className="w-full">
                        <CardHeader className="pb-3">
                            <div className="flex items-center gap-2">
                                <FontAwesomeIcon
                                    icon={faDownload}
                                    className="text-success text-lg"
                                />
                                <h2 className="text-xl font-semibold">
                                    Installation
                                </h2>
                            </div>
                        </CardHeader>
                        <CardBody className="pt-0">
                            <p className="text-default-600 mb-3">
                                Add the following line to your{" "}
                                <code className="text-sm bg-default-100 px-1 py-0.5 rounded">
                                    cabin.toml
                                </code>{" "}
                                file:
                            </p>
                            <Code
                                size="lg"
                                className="w-full"
                            >{`"${pack.name}" = "${pack.version}"`}</Code>
                        </CardBody>
                    </Card>

                    {/* README */}
                    <Card className="w-full">
                        <CardHeader className="pb-3">
                            <div className="flex items-center gap-2">
                                <FontAwesomeIcon
                                    icon={faBookOpen}
                                    className="text-primary text-lg"
                                />
                                <h2 className="text-xl font-semibold">
                                    README
                                </h2>
                            </div>
                        </CardHeader>
                        <CardBody className="pt-0">
                            {pack.readme ? (
                                <div className="prose dark:prose-invert max-w-none">
                                    <ReactMarkdown skipHtml>
                                        {pack.readme}
                                    </ReactMarkdown>
                                </div>
                            ) : (
                                <div className="text-center py-8">
                                    <FontAwesomeIcon
                                        icon={faBookOpen}
                                        className="text-default-300 text-3xl mb-3"
                                    />
                                    <p className="text-default-500">
                                        No README available for this package
                                    </p>
                                </div>
                            )}
                        </CardBody>
                    </Card>
                </div>

                {/* Sidebar */}
                <div className="space-y-6">
                    {/* Package Stats */}
                    <Card>
                        <CardHeader className="pb-3">
                            <h3 className="text-lg font-semibold">
                                Package Info
                            </h3>
                        </CardHeader>
                        <CardBody className="pt-0 space-y-4">
                            <div className="flex items-center justify-between">
                                <div className="flex items-center gap-2">
                                    <FontAwesomeIcon
                                        icon={faBox}
                                        className="text-default-500"
                                    />
                                    <span className="text-default-600">
                                        Versions
                                    </span>
                                </div>
                                <Chip size="sm" variant="flat">
                                    {numVersion}
                                </Chip>
                            </div>
                            <div className="flex items-center justify-between">
                                <div className="flex items-center gap-2">
                                    <FontAwesomeIcon
                                        icon={faCodeBranch}
                                        className="text-default-500"
                                    />
                                    <span className="text-default-600">
                                        Dependencies
                                    </span>
                                </div>
                                <Chip size="sm" variant="flat">
                                    {pack.metadata.dependencies?.length ?? 0}
                                </Chip>
                            </div>
                            <div className="flex items-center justify-between">
                                <div className="flex items-center gap-2">
                                    <FontAwesomeIcon
                                        icon={faCalendarAlt}
                                        className="text-default-500"
                                    />
                                    <span className="text-default-600">
                                        Published
                                    </span>
                                </div>
                                <Chip size="sm" variant="flat">
                                    {format(pack.published_at)}
                                </Chip>
                            </div>
                            <div className="flex items-center justify-between">
                                <div className="flex items-center gap-2">
                                    <FontAwesomeIcon
                                        icon={faGavel}
                                        className="text-default-500"
                                    />
                                    <span className="text-default-600">
                                        License
                                    </span>
                                </div>
                                <Link
                                    isExternal
                                    href={`https://choosealicense.com/licenses/${pack.license.toLowerCase()}/`}
                                    size="sm"
                                >
                                    {pack.license}
                                </Link>
                            </div>
                        </CardBody>
                    </Card>

                    {/* Links */}
                    <Card>
                        <CardHeader className="pb-3">
                            <h3 className="text-lg font-semibold">Links</h3>
                        </CardHeader>
                        <CardBody className="pt-0 space-y-3">
                            {pack.metadata.package.homepage && (
                                <Button
                                    as={Link}
                                    href={pack.metadata.package.homepage}
                                    isExternal
                                    variant="flat"
                                    className="w-full justify-start"
                                    startContent={
                                        <FontAwesomeIcon
                                            icon={faHome}
                                            className="text-sm"
                                        />
                                    }
                                >
                                    Homepage
                                </Button>
                            )}
                            {pack.metadata.package.documentation && (
                                <Button
                                    as={Link}
                                    href={pack.metadata.package.documentation}
                                    isExternal
                                    variant="flat"
                                    className="w-full justify-start"
                                    startContent={
                                        <FontAwesomeIcon
                                            icon={faBookOpen}
                                            className="text-sm"
                                        />
                                    }
                                >
                                    Documentation
                                </Button>
                            )}
                            {pack.metadata.package.repository && (
                                <Button
                                    as={Link}
                                    href={pack.metadata.package.repository}
                                    isExternal
                                    variant="flat"
                                    className="w-full justify-start"
                                    startContent={
                                        <FontAwesomeIcon
                                            icon={faCodeBranch}
                                            className="text-sm"
                                        />
                                    }
                                >
                                    Repository
                                </Button>
                            )}
                        </CardBody>
                    </Card>
                </div>
            </div>
        </div>
    );
}
