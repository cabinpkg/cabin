module Algolia exposing (performSearchIndex)

import Http
import Json.Decode
import Json.Decode.Pipeline
import Json.Encode
import Messages exposing (Msg(..))
import Model exposing (Algolia, Package, SearchResponse)
import Url



-- Decoder


searchResponseDecoder : Json.Decode.Decoder SearchResponse
searchResponseDecoder =
    Json.Decode.succeed SearchResponse
        |> Json.Decode.Pipeline.required "hits" searchHitListDecoder
        |> Json.Decode.Pipeline.required "nbHits" Json.Decode.int
        |> Json.Decode.Pipeline.required "nbPages" Json.Decode.int


searchHitListDecoder : Json.Decode.Decoder (List Package)
searchHitListDecoder =
    Json.Decode.list packageSearchHitDecoder


packageSearchHitDecoder : Json.Decode.Decoder Package
packageSearchHitDecoder =
    Json.Decode.succeed Package
        |> Json.Decode.Pipeline.requiredAt [ "package", "name" ] Json.Decode.string
        |> Json.Decode.Pipeline.requiredAt [ "_highlightResult", "package", "name", "value" ] Json.Decode.string
        |> Json.Decode.Pipeline.requiredAt [ "package", "version" ] Json.Decode.string
        |> Json.Decode.Pipeline.requiredAt [ "package", "description" ] Json.Decode.string
        |> Json.Decode.Pipeline.requiredAt [ "package", "repository" ] Json.Decode.string



-- Algolia Index Search


searchIndexUrl : String -> String -> Url.Url
searchIndexUrl algoliaAppId indexName =
    { protocol = Url.Https
    , host = algoliaAppId ++ "-dsn.algolia.net"
    , port_ = Nothing
    , path = "/1/indexes/" ++ indexName ++ "/query"
    , query = Nothing
    , fragment = Nothing
    }


searchIndexBody : String -> Int -> Int -> Json.Encode.Value
searchIndexBody searchString searchCount pageNumber =
    let
        query : String
        query =
            "query=" ++ searchString

        hitsPerPage : String
        hitsPerPage =
            "hitsPerPage=" ++ String.fromInt searchCount

        distinct : String
        distinct =
            "distinct=true"

        page : String
        page =
            "page=" ++ String.fromInt pageNumber

        queryString : String
        queryString =
            String.join "&"
                [ query
                , hitsPerPage
                , distinct
                , page
                ]

        indexObjects : Json.Encode.Value
        indexObjects =
            Json.Encode.string queryString
    in
    Json.Encode.object
        [ ( "params"
          , indexObjects
          )
        ]


performSearchIndex : Algolia -> String -> Int -> Int -> Cmd Msg
performSearchIndex algolia searchString searchCount pageNumber =
    let
        url : Url.Url
        url =
            searchIndexUrl algolia.applicationId algolia.indexName

        body : Http.Body
        body =
            Http.jsonBody (searchIndexBody searchString searchCount pageNumber)

        headers : List Http.Header
        headers =
            [ Http.header "X-Algolia-API-Key" algolia.apiKey
            , Http.header "X-Algolia-Application-Id" algolia.applicationId
            ]
    in
    Http.request
        { method = "POST"
        , headers = headers
        , url = url |> Url.toString
        , body = body
        , expect = Http.expectJson ReceivePackages searchResponseDecoder
        , timeout = Nothing
        , tracker = Nothing
        }
