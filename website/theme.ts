import { extendTheme } from "@chakra-ui/react";

const config = {
    initialColorMode: "dark",
    useSystemColorMode: true,
};

const theme = extendTheme(config);

export default theme;
