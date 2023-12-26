import {
    Code,
    HStack,
    Heading,
    ListItem,
    Tab,
    TabList,
    TabPanel,
    TabPanels,
    Tabs,
    Tag,
    Text,
    UnorderedList,
    VStack,
} from "@chakra-ui/react";
import {
    faArrowUpRightFromSquare,
    faFileLines,
    faLink,
    faTags,
} from "@fortawesome/free-solid-svg-icons";
import { FontAwesomeIcon } from "@fortawesome/react-fontawesome";
import ChakraUIRenderer from "chakra-ui-markdown-renderer";
import ReactMarkdown from "react-markdown";
import remarkGfm from "remark-gfm";

import { CodeBlock } from "~/components/CodeBlock";
import InfoColumn from "~/components/InfoColumn";
import { Link } from "~/components/Link";
import type { Package } from "~/utils/types";

interface InfoMainProps {
    package: Package;
    versions: string[];
    dependents: Package[];
}

function InfoMain(props: InfoMainProps): JSX.Element {
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
                        <FontAwesomeIcon
                            icon={faArrowUpRightFromSquare}
                            width={15}
                        />
                        {props.package.metadata.dependencies ? (
                            // @ts-ignore
                            <Code>
                                {
                                    Object.keys(
                                        props.package.metadata.dependencies,
                                    ).length
                                }
                            </Code>
                        ) : (
                            <Code>0</Code>
                        )}
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
                    {props.package.readme ? (
                        <ReactMarkdown
                            remarkPlugins={[remarkGfm]}
                            components={ChakraUIRenderer({
                                code: CodeBlock,
                            })}
                            skipHtml
                        >
                            {props.package.readme}
                        </ReactMarkdown>
                    ) : (
                        <Text>no readme found</Text>
                    )}
                </TabPanel>
                <TabPanel>
                    <UnorderedList>
                        {props.versions.map((v) => (
                            <ListItem key={v}>
                                <Link
                                    href={`/packages/${props.package.name}/${v}`}
                                >
                                    {v}
                                </Link>
                            </ListItem>
                        ))}
                    </UnorderedList>
                </TabPanel>
                <TabPanel>
                    {props.package.metadata.dependencies ? (
                        <UnorderedList>
                            {Object.entries(
                                props.package.metadata.dependencies,
                            ).map(([name, ver]) => (
                                // @ts-ignore
                                <ListItem key={name}>
                                    {/* @ts-ignore */}
                                    {name}: {ver}
                                </ListItem>
                            ))}
                        </UnorderedList>
                    ) : (
                        <Text>This package has no dependencies.</Text>
                    )}
                </TabPanel>
                <TabPanel>
                    {props.dependents && props.dependents.length > 0 ? (
                        <UnorderedList>
                            {props.dependents.map((d) => (
                                <ListItem key={d.id}>
                                    {d.name}: {d.version}
                                </ListItem>
                            ))}
                        </UnorderedList>
                    ) : (
                        <Text>
                            This package is not used as a dependency yet.
                        </Text>
                    )}
                </TabPanel>
            </TabPanels>
        </Tabs>
    );
}

interface PackageHeadingProps {
    package: Package;
}

function PackageHeading(props: PackageHeadingProps): JSX.Element {
    return (
        <VStack spacing={5}>
            <HStack spacing={5}>
                <Heading>{props.package.name}</Heading>
                <Heading>v{props.package.version}</Heading>
                <Tag>C++{props.package.edition.toString().slice(-2)}</Tag>
            </HStack>
            <Heading size="md">{props.package.description}</Heading>
        </VStack>
    );
}

interface PackageDetailsProps {
    package: Package;
    versions: string[];
    dependents: Package[];
}

export default function PackageDetails(
    props: PackageDetailsProps,
): JSX.Element {
    return (
        <VStack spacing={5}>
            <PackageHeading package={props.package} />
            <HStack spacing={5} alignItems="start">
                <InfoMain
                    package={props.package}
                    versions={props.versions}
                    dependents={props.dependents}
                />
                <InfoColumn package={props.package} />
            </HStack>
        </VStack>
    );
}
