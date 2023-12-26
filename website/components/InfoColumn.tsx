import { CalendarIcon, LinkIcon } from "@chakra-ui/icons";
import {
    Avatar,
    Button,
    Code,
    HStack,
    StackDivider,
    Text,
    VStack,
    useClipboard,
} from "@chakra-ui/react";
import { faGithub } from "@fortawesome/free-brands-svg-icons";
import {
    faClipboard,
    faClipboardCheck,
    faFileCode,
    faScaleBalanced,
} from "@fortawesome/free-solid-svg-icons";
import { FontAwesomeIcon } from "@fortawesome/react-fontawesome";
import { useEffect, useState } from "react";
import { format } from "timeago.js";
import InfoColumnItem from "~/components/InfoColumnItem";
import { Link } from "~/components/Link";
import { Package, User } from "~/utils/types";

interface Props {
    package: Package;
}

const Metadata = (props: Props) => (
    <InfoColumnItem title="Metadata">
        <HStack>
            <CalendarIcon />
            <Text>{format(props.package.published_at)}</Text>
        </HStack>
        <HStack>
            <FontAwesomeIcon icon={faScaleBalanced} width={20} />
            <Link
                href={`https://choosealicense.com/licenses/${props.package.license.toLowerCase()}/`}
                isExternal
            >
                {props.package.license}
            </Link>
        </HStack>
    </InfoColumnItem>
);

const Install = (props: Props) => {
    const installSnippet = `"${props.package.name}" = "${props.package.version}"`;
    const { hasCopied, onCopy } = useClipboard(installSnippet);

    return (
        <InfoColumnItem title="Install">
            <Text fontSize="xs">
                Add the following line to your{" "}
                <Code fontSize="xs">poac.toml</Code> file:
            </Text>
            <Button
                onClick={onCopy}
                rightIcon={
                    hasCopied ? (
                        <FontAwesomeIcon icon={faClipboardCheck} width={15} />
                    ) : (
                        <FontAwesomeIcon icon={faClipboard} width={15} />
                    )
                }
            >
                {installSnippet}
            </Button>
        </InfoColumnItem>
    );
};

export default function InfoColumn(props: Props): JSX.Element {
    const installSnippet = `"${props.package.name}" = "${props.package.version}"`;

    const { hasCopied, onCopy } = useClipboard(installSnippet);
    const [owners, setOwners] = useState<User[]>([]);

    // useEffect(() => {
    //     fetch(`${BASE_API_URL}/packages/${props.package.name}/owners`)
    //         .then((res) => {
    //             res.json().then((data) => {
    //                 const users: User[] = [];
    //                 for (const rawUser of data.data) {
    //                     users.push({
    //                         id: rawUser.id,
    //                         name: rawUser.name,
    //                         user_name: rawUser.user_name,
    //                         avatar_url: rawUser.avatar_url,
    //                     });
    //                 }
    //                 setOwners(users);
    //             });
    //         })
    //         .catch((err) => console.error(err));
    // }, [props.package.name]);

    return (
        <VStack spacing={5} maxWidth={300} divider={<StackDivider />}>
            <Metadata package={props.package} />
            <Install package={props.package} />
            {props.package.metadata.package.homepage && (
                <InfoColumnItem title="Homepage">
                    <HStack>
                        <LinkIcon />
                        <Link
                            href={props.package.metadata.package.homepage}
                            isExternal
                        >
                            {props.package.metadata.package.homepage}
                        </Link>
                    </HStack>
                </InfoColumnItem>
            )}
            {props.package.metadata.package.documentation && (
                <InfoColumnItem title="Documentation">
                    <HStack>
                        <FontAwesomeIcon icon={faFileCode} width={15} />
                        <Link
                            href={props.package.metadata.package.documentation}
                            isExternal
                        >
                            {props.package.metadata.package.documentation}
                        </Link>
                    </HStack>
                </InfoColumnItem>
            )}
            <InfoColumnItem title="Repository">
                <HStack>
                    <FontAwesomeIcon icon={faGithub} width={20} />
                    <Link href={props.package.repository} isExternal>
                        {props.package.repository.replace(
                            "https://github.com/",
                            "",
                        )}
                    </Link>
                </HStack>
            </InfoColumnItem>
            <InfoColumnItem title="Owners" count={owners.length}>
                {owners.map((o) => (
                    <HStack key={o.id}>
                        <Avatar size="xs" name={o.name} src={o.avatar_url} />
                        <Link href={`/users/${o.user_name}`}>{o.name}</Link>
                    </HStack>
                ))}
            </InfoColumnItem>
        </VStack>
    );
}
