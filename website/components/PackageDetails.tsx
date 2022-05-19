import {
    Avatar,
    Code, Divider,
    Heading,
    HStack, Link, ListItem,
    Tab,
    TabList,
    TabPanel,
    TabPanels,
    Tabs, Tag,
    Text,
    UnorderedList,
    VStack,
    Button,
    useClipboard } from "@chakra-ui/react";
import { FontAwesomeIcon } from "@fortawesome/react-fontawesome";
import { faArrowUpRightFromSquare, faFileLines, faLink, faTags, faScaleBalanced, faClipboard, faClipboardCheck, faFileCode } from "@fortawesome/free-solid-svg-icons";
import { faGithub } from "@fortawesome/free-brands-svg-icons";
import { useEffect, useState } from "react";
import { supabaseClient } from "@supabase/supabase-auth-helpers/nextjs";
import { format } from "timeago.js";
import { CalendarIcon, LinkIcon } from "@chakra-ui/icons";
import ReactMarkdown from "react-markdown";
import ChakraUIRenderer from "chakra-ui-markdown-renderer";
import remarkGfm from "remark-gfm";

import type { Package as PackageType, User } from "~/utils/types";

interface PackageSubProps {
    package: PackageType;
}

function PackageSub(props: PackageSubProps): JSX.Element {
    const installSnippet = `"${props.package.name}" = "${props.package.version}"`;

    const { hasCopied, onCopy } = useClipboard(installSnippet);
    const [owners, setOwners] = useState<User[]>([]);

    useEffect(() => {
        supabaseClient
            .rpc<User>("get_owners", { pkgname: props.package.name })
            .select("*")
            .then(({ data }) => {
                if (data) {
                    setOwners(data);
                }
            });
    }, [props.package.name]);

    return (
        <VStack spacing={10} maxWidth={300}>
            <VStack width="100%">
                <Text as="b">Metadata</Text>
                <Divider />
                <HStack>
                    <CalendarIcon />
                    <Text>{format(props.package.published_at)}</Text>
                </HStack>
                <HStack>
                    <FontAwesomeIcon icon={faScaleBalanced} width={20} />
                    <Text>{props.package.license}</Text>
                </HStack>
            </VStack>
            <VStack width="100%">
                <Text as="b">Install</Text>
                <Divider />
                <Text fontSize="xs">Add the following line to your <Code fontSize="xs">poac.toml</Code> file:</Text>
                <Button
                    onClick={onCopy}
                    rightIcon={
                        hasCopied ?
                            <FontAwesomeIcon icon={faClipboardCheck} width={15} /> :
                            <FontAwesomeIcon icon={faClipboard} width={15} />
                    }
                >
                    {installSnippet}
                </Button>
            </VStack>
            {props.package.metadata["package"]["homepage"] &&
                <VStack width="100%">
                    <Text as="b">Homepage</Text>
                    <Divider />
                    <HStack>
                        <LinkIcon />
                        <Link href={props.package.metadata["package"]["homepage"]} isExternal>
                            {props.package.metadata["package"]["homepage"]}
                        </Link>
                    </HStack>
                </VStack>
            }
            {props.package.metadata["package"]["documentation"] &&
                <VStack width="100%">
                    <Text as="b">Documentation</Text>
                    <Divider />
                    <HStack>
                        <FontAwesomeIcon icon={faFileCode} width={15} />
                        <Link href={props.package.metadata["package"]["documentation"]} isExternal>
                            {props.package.metadata["package"]["documentation"]}
                        </Link>
                    </HStack>
                </VStack>
            }
            <VStack width="100%">
                <Text as="b">Repository</Text>
                <Divider />
                <HStack>
                    <FontAwesomeIcon icon={faGithub} width={20} />
                    <Link href={props.package.repository} isExternal>
                        {props.package.repository.replace("https://github.com/", "")}
                    </Link>
                </HStack>
            </VStack>
            <VStack width="100%">
                <Text as="b">Owners</Text>
                <Divider />
                {owners.map((o) =>
                    <HStack key={o.id}>
                        <Avatar size="xs" name={o.name} src={o.avatar_url} />
                        <Link href={`/users/${o.user_name}`}>{o.name}</Link>
                    </HStack>
                )}
            </VStack>
        </VStack>
    );
}

interface PackageMainProps {
    package: PackageType;
    versions: string[];
    dependents: PackageType[];
}

function PackageMain(props: PackageMainProps): JSX.Element {
    return (
        <Tabs maxWidth={700}>
            <TabList>
                <Tab>
                    <HStack spacing={2}>
                        <FontAwesomeIcon icon={faFileLines} width={13} />
                        <Text>Readme</Text>
                    </HStack>
                </Tab>
                <Tab>
                    <HStack spacing={1}>
                        <FontAwesomeIcon icon={faTags} width={18} />
                        <Code>{props.versions.length}</Code>
                        <Text>Versions</Text>
                    </HStack>
                </Tab>
                <Tab>
                    <HStack spacing={1}>
                        <FontAwesomeIcon icon={faArrowUpRightFromSquare} width={15} />
                        {props.package.metadata["dependencies"] ?
                            // eslint-disable-next-line @typescript-eslint/ban-ts-comment
                            // @ts-ignore
                            <Code>{Object.keys(props.package.metadata["dependencies"]).length}</Code> :
                            <Code>0</Code>
                        }
                        <Text>Dependencies</Text>
                    </HStack>
                </Tab>
                <Tab>
                    <HStack spacing={1}>
                        <FontAwesomeIcon icon={faLink} width={20} />
                        <Code>{props.dependents.length}</Code>
                        <Text>Dependents</Text>
                    </HStack>
                </Tab>
            </TabList>

            <TabPanels>
                <TabPanel>
                    {props.package.readme ?
                        <ReactMarkdown remarkPlugins={[remarkGfm]} components={ChakraUIRenderer()} skipHtml>
                            {props.package.readme}
                        </ReactMarkdown> :
                        <Text>no readme found</Text>
                    }
                </TabPanel>
                <TabPanel>
                    <UnorderedList>
                        {props.versions.map((v) => <ListItem key={v}>
                            <Link href={`/packages/${props.package.name}/${v}`}>{v}</Link>
                        </ListItem>)}
                    </UnorderedList>
                </TabPanel>
                <TabPanel>
                    {props.package.metadata["dependencies"] ?
                        <UnorderedList>
                            {/* eslint-disable-next-line @typescript-eslint/ban-ts-comment */}
                            {/* @ts-ignore */}
                            {Object.entries(props.package.metadata["dependencies"]).map(([name, ver]) => <ListItem key={name}>{name}: {ver}</ListItem>)}
                        </UnorderedList> :
                        <Text>This package has no dependencies.</Text>
                    }
                </TabPanel>
                <TabPanel>
                    {props.dependents && props.dependents.length > 0 ?
                        <UnorderedList>
                            {props.dependents.map((d) => <ListItem key={d.id}>{d.name}: {d.version}</ListItem>)}
                        </UnorderedList> :
                        <Text>This package is not used as a dependency yet.</Text>
                    }
                </TabPanel>
            </TabPanels>
        </Tabs>
    );
}

interface PackageHeadingProps {
    package: PackageType;
}

function PackageHeading(props: PackageHeadingProps): JSX.Element {
    return (
        <VStack spacing={5}>
            <HStack spacing={5}>
                <Heading>{props.package.name}</Heading>
                <Heading>v{props.package.version}</Heading>
                <Tag>C++{props.package.edition.toString().slice(-2)}</Tag>
            </HStack>
            <Heading size="md">
                {props.package.description}
            </Heading>
        </VStack>
    );
}

interface PackageDetailsProps {
    package: PackageType;
    versions: string[];
    dependents: PackageType[];
}

export default function PackageDetails(props: PackageDetailsProps): JSX.Element {
    return (
        <VStack spacing={5}>
            <PackageHeading package={props.package} />
            <HStack spacing={5} alignItems="start">
                <PackageMain package={props.package} versions={props.versions} dependents={props.dependents} />
                <PackageSub package={props.package} />
            </HStack>
        </VStack>
    );
}
