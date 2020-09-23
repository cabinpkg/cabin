module Algolia.Api exposing (performSearchIndex)

import Algolia.Decoder exposing (searchResponseDecoder)
import Http
import Json.Encode
import Messages exposing (Msg(..))
import Model exposing (Algolia)
import Url


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
            searchCount
                |> String.fromInt
                |> (++) "hitsPerPage="

        distinct : String
        distinct =
            "distinct=true"

        page : String
        page =
            pageNumber
                - 1
                |> String.fromInt
                |> (++) "page="

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
