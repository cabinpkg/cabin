module Update exposing (loadCurrentPage, update)

import Algolia.Api exposing (performSearchIndex)
import Browser
import Browser.Dom exposing (getViewport, setViewport)
import Browser.Navigation as Nav
import Messages exposing (..)
import Model exposing (IsFadein, Model, SearchInfo, initSearchInfo)
import Route
import Style.Theme
import Task
import Url


update : Msg -> Model -> ( Model, Cmd Msg )
update msg model =
    case msg of
        OnUrlRequest urlRequest ->
            case urlRequest of
                Browser.Internal url ->
                    ( model
                    , Nav.pushUrl model.navKey (Url.toString url)
                    )

                Browser.External href ->
                    ( model
                    , Nav.load href
                    )

        OnUrlChange url ->
            { model | route = Route.fromUrl url }
                |> loadCurrentPage
                |> resetViewport

        OnAnimationFrame _ ->
            ( model, Task.perform GotNewViewport getViewport )

        OnThemeChange isDarkTheme ->
            ( { model | theme = Style.Theme.select isDarkTheme }
            , Cmd.none
            )

        GotNewViewport viewport ->
            if viewport.viewport.y > 600 then
                ( setTarget asSection1In model True, Cmd.none )

            else if viewport.viewport.y > 200 then
                ( setTarget asGetStartIn model True, Cmd.none )

            else
                ( model, Cmd.none )

        GotNewWidth width ->
            ( { model | width = width }, Cmd.none )

        OnSearchInput searchCount searchInput ->
            let
                oldSearchInfo : SearchInfo
                oldSearchInfo =
                    model.searchInfo

                newSearchInfo : SearchInfo
                newSearchInfo =
                    { oldSearchInfo | currentPage = 1 }
            in
            ( { model
                | searchInput = searchInput
                , searchInfo = newSearchInfo
              }
            , performSearchIndex model.algolia searchInput searchCount 1
            )

        OnEnterPress ->
            ( model, Route.replaceUrl model.navKey (Route.Packages Nothing) )

        ReceivePackages searchResult ->
            case searchResult of
                Ok result ->
                    let
                        oldSearchInfo : SearchInfo
                        oldSearchInfo =
                            model.searchInfo

                        newSearchInfo : SearchInfo
                        newSearchInfo =
                            { oldSearchInfo
                                | countHits = result.nbHits
                                , countPages = result.nbPages
                            }
                    in
                    ( { model
                        | packages = result.hits
                        , searchInfo = newSearchInfo
                      }
                    , Cmd.none
                    )

                Err _ ->
                    ( { model
                        | packages = []
                        , searchInfo = initSearchInfo
                      }
                    , Cmd.none
                    )

        ClearPackages ->
            if model.searchInput == "" then
                ( { model
                    | packages = []
                    , searchInfo = initSearchInfo
                  }
                , Cmd.none
                )

            else
                ( model, Cmd.none )

        NoOp ->
            ( model, Cmd.none )


loadCurrentPage : Model -> ( Model, Cmd Msg )
loadCurrentPage model =
    case model.route of
        Route.Home ->
            ( { model | packages = [] }, Cmd.none )

        Route.Packages page ->
            let
                currentPage : Int
                currentPage =
                    -- If nothing, the page number will set 1 (first page)
                    Maybe.withDefault 1 page

                oldSearchInfo : SearchInfo
                oldSearchInfo =
                    model.searchInfo

                newSearchInfo : SearchInfo
                newSearchInfo =
                    { oldSearchInfo | currentPage = currentPage }
            in
            ( { model | searchInfo = newSearchInfo }
                |> turnOffFadein
            , performSearchIndex model.algolia model.searchInput 20 currentPage
            )

        _ ->
            ( turnOffFadein model, Cmd.none )


resetViewport : ( Model, Cmd Msg ) -> ( Model, Cmd Msg )
resetViewport ( model, msg ) =
    ( model
    , Cmd.batch
        [ msg
        , Task.perform (\_ -> NoOp) (setViewport 0 0)
        ]
    )


setSection1 : Bool -> IsFadein -> IsFadein
setSection1 newBool isFadein =
    { isFadein | section1 = newBool }


asSection1In : IsFadein -> Bool -> IsFadein
asSection1In =
    \b a -> setSection1 a b


setGetStart : Bool -> IsFadein -> IsFadein
setGetStart newBool isFadein =
    { isFadein | getStart = newBool }


asGetStartIn : IsFadein -> Bool -> IsFadein
asGetStartIn =
    \b a -> setGetStart a b


setTarget : (IsFadein -> Bool -> IsFadein) -> Model -> Bool -> Model
setTarget asInFn model newBool =
    newBool
        |> asInFn model.isFadein
        |> asIsFadein model


setIsFadein : IsFadein -> Model -> Model
setIsFadein newIsFadein model =
    { model | isFadein = newIsFadein }


asIsFadein : Model -> IsFadein -> Model
asIsFadein =
    \b a -> setIsFadein a b


setAll : Bool -> IsFadein -> IsFadein
setAll newBool isFadein =
    isFadein
        |> setSection1 newBool
        |> setGetStart newBool


asAll : IsFadein -> Bool -> IsFadein
asAll =
    \b a -> setAll a b


setAllIsFadein : Bool -> Model -> Model
setAllIsFadein newIsFadein model =
    newIsFadein
        |> asAll model.isFadein
        |> asIsFadein model


turnOffFadein : Model -> Model
turnOffFadein model =
    setAllIsFadein False model
