import {
    Code,
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
} from "@chakra-ui/react";
import { FontAwesomeIcon } from "@fortawesome/react-fontawesome";
import { faArrowUpRightFromSquare, faFileLines, faLink, faTags } from "@fortawesome/free-solid-svg-icons";

import type { Package as PackageType } from "~/utils/types";

interface PackageDetailsProps {
    package: PackageType;
    versions: string[];
    dependents: PackageType[];
}

export default function PackageDetails(props: PackageDetailsProps): JSX.Element {
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

            <Tabs>
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
                        <Text>no readme found</Text>
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
        </VStack>
    );
}
