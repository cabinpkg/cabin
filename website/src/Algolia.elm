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


searchHitListDecoder : Json.Decode.Decoder (List Package)
searchHitListDecoder =
    Json.Decode.list packageSearchHitDecoder


packageSearchHitDecoder : Json.Decode.Decoder Package
packageSearchHitDecoder =
    Json.Decode.succeed Package
        |> Json.Decode.Pipeline.requiredAt [ "package", "name" ] Json.Decode.string
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
        query =
            "query=" ++ searchString

        hitsPerPage =
            "hitsPerPage=" ++ String.fromInt searchCount

        distinct =
            "distinct=true"

        page =
            "page=" ++ String.fromInt pageNumber

        queryString =
            String.join "&"
                [ query
                , hitsPerPage
                , distinct
                , page
                ]

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
        url =
            searchIndexUrl algolia.applicationId algolia.indexName

        body =
            Http.jsonBody (searchIndexBody searchString searchCount pageNumber)

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
