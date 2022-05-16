import {
    Code,
    Heading,
    HStack, Link, ListItem,
    Tab,
    TabList,
    TabPanel,
    TabPanels,
    Tabs,
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
            </HStack>
            <Heading size="md">
                {props.package.description}
            </Heading>

            <Tabs>
                <TabList>
                    <Tab>
                        <FontAwesomeIcon icon={faFileLines} />
                        <Text marginLeft={2}>Readme</Text>
                    </Tab>
                    <Tab>
                        <FontAwesomeIcon icon={faTags} />
                        <Text marginLeft={2}><Code>{props.versions.length}</Code> Versions</Text>
                    </Tab>
                    <Tab>
                        <FontAwesomeIcon icon={faArrowUpRightFromSquare} />
                        {props.package.metadata["dependencies"] ?
                            // eslint-disable-next-line @typescript-eslint/ban-ts-comment
                            // @ts-ignore
                            <Code marginLeft={2}>{Object.keys(props.package.metadata["dependencies"]).length}</Code> :
                            <Code marginLeft={2}>0</Code>
                        }
                        <Text marginLeft={1}>Dependencies</Text>
                    </Tab>
                    <Tab>
                        <FontAwesomeIcon icon={faLink} />
                        <Text marginLeft={2}><Code>{props.dependents.length}</Code> Dependents</Text>
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
