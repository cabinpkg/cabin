module Main exposing (init, main)

import Browser
import Browser.Navigation exposing (Key)
import Messages exposing (Msg(..))
import Model exposing (Model, Flags, RemoteData(..))
import Route
import Update exposing (update, loadCurrentPage)
import Page exposing (view)
import Url exposing (Url)
import Subscriptions exposing (subscriptions)


init : Flags -> Url -> Key -> ( Model, Cmd Msg )
init flags url navKey =
    let
        model =
            { flags = flags
            , navKey = navKey
            , route = Route.fromUrl url
            , ownPackages = NotRequested
            , packageVersions = NotRequested
            , package = NotRequested
            , search = ""
            , isFadein =
                { abstract = False
                , section1 = False
                , demo = False
                , getStart = False
                }
            , searchInput = ""
            , isChecked = False
            }
    in
    model
        |> loadCurrentPage


main : Program Flags Model Msg
main =
    Browser.application
        { init = init
        , view = view
        , update = update
        , subscriptions = subscriptions
        , onUrlRequest = OnUrlRequest
        , onUrlChange = OnUrlChange
        }
