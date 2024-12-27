import { Chip, Code, Divider, Link } from "@nextui-org/react";
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
        <div className="flex flex-col justify-center items-center gap-4 m-4">
            <div className="flex justify-center items-center gap-4">
                <h1 className="text-3xl font-bold">{pack.name}</h1>
                <h2 className="text-lg text-white/50">v{pack.version}</h2>
                <Chip>C++{pack.edition.toString().slice(-2)}</Chip>
            </div>
            <h3 className="text-xl text-white/50">{pack.description}</h3>
            <div className="flex flex-col lg:flex-row justify-start gap-4 items-start">
                <div className="flex flex-row justify-center gap-1 max-w-[200px]">
                    <Chip>{numVersion}</Chip>
                    <h4 className="text-lg font-bold">Versions</h4>
                </div>
                <div className="flex flex-row justify-center gap-1 max-w-[200px]">
                    <Chip>{pack.metadata.dependencies?.length ?? 0}</Chip>
                    <h4 className="text-lg font-bold">Dependencies</h4>
                </div>
                <div className="flex flex-col justify-center gap-1 max-w-[200px]">
                    <h4 className="text-lg font-bold">Metadata</h4>
                    <Chip>{format(pack.published_at)}</Chip>
                    <Link
                        isExternal
                        href={`https://choosealicense.com/licenses/${pack.license.toLowerCase()}/`}
                    >
                        {pack.license}
                    </Link>
                </div>
                <div className="flex flex-col justify-center gap-1 max-w-[200px] break-words">
                    <h4 className="text-lg font-bold">Install</h4>
                    <p className="text-xs">
                        Add the following line to your cabin.toml file:
                    </p>
                    <Code>{`"${pack.name}" = "${pack.version}"`}</Code>
                </div>
                {pack.metadata.package.homepage && (
                    <div className="flex flex-col justify-center gap-1 max-w-[200px] break-all">
                        <h4 className="text-lg font-bold">Homepage</h4>
                        <Link isExternal href={pack.metadata.package.homepage}>
                            {pack.metadata.package.homepage}
                        </Link>
                    </div>
                )}
                {pack.metadata.package.documentation && (
                    <div className="flex flex-col justify-center gap-1 max-w-[200px] break-all">
                        <h4 className="text-lg font-bold">Documentation</h4>
                        <Link
                            isExternal
                            href={pack.metadata.package.documentation}
                        >
                            {pack.metadata.package.documentation}
                        </Link>
                    </div>
                )}
                {pack.metadata.package.repository && (
                    <div className="flex flex-col justify-center gap-1 max-w-[200px] break-all">
                        <h4 className="text-lg font-bold">Repository</h4>
                        <Link
                            isExternal
                            href={pack.metadata.package.repository}
                        >
                            {pack.metadata.package.repository}
                        </Link>
                    </div>
                )}
            </div>
            <Divider />
            {pack.readme ? (
                <ReactMarkdown className="prose dark:prose-invert" skipHtml>
                    {pack.readme}
                </ReactMarkdown>
            ) : (
                <p className="text-white/50">no readme found</p>
            )}
        </div>
    );
}
