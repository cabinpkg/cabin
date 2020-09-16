module Main exposing (init, main)

import Browser
import Browser.Navigation exposing (Key)
import Messages exposing (Msg(..))
import Model exposing (Flags, Model)
import Page exposing (toUnstyledDocument, view)
import Route
import Subscriptions exposing (subscriptions)
import Update exposing (loadCurrentPage, update)
import Url exposing (Url)


init : Flags -> Url -> Key -> ( Model, Cmd Msg )
init flags url navKey =
    let
        model =
            { navKey = navKey
            , route = Route.fromUrl url
            , width = flags.width
            , algolia =
                { apiKey = flags.algoliaApiKey
                , applicationId = flags.algoliaApplicationId
                , indexName = flags.algoliaIndexName
                }
            , isFadein =
                { section1 = False
                , getStart = False
                }
            , searchInput = ""
            , packages = []
            }
    in
    model
        |> loadCurrentPage


main : Program Flags Model Msg
main =
    Browser.application
        { init = init
        , view = view >> toUnstyledDocument
        , update = update
        , subscriptions = subscriptions
        , onUrlRequest = OnUrlRequest
        , onUrlChange = OnUrlChange
        }
