export const PER_PAGE = 10;
export const BASE_URL =
    process.env.NODE_ENV === "development"
        ? "http://localhost:3000"
        : "https://poac.dev";
export const BASE_API_URL = "https://poac-api.shuttleapp.rs/v1";
// process.env.NODE_ENV === "development"
//     ? "http://127.0.0.1:8000/v1"
//     : "https://poac-api.shuttleapp.rs/v1";
